//! Server hosting features

//use std::fs::{File, remove_file};
use std::io::{Result, Read, Write};
//use std::io::{stdout, stdin};
use std::os::unix::net::{UnixListener};
use std::net::TcpListener;
use std::sync::{RwLock, Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::process;

use protobuf::{Message, CodedInputStream};

use std::collections::{HashMap};
use super::rics;


/// Contains server permanent state
pub struct ServerState {
    /// Flag for if the CAN broadcasting is enabled
    can_broadcast: bool,
    /// Internal flag for node id allocation
    node_allocator: i32,
    /// Holds the self described names of the nodes
    node_names: HashMap<i32, String>,
    /// Input socket handle
    node_inputs: HashMap<i32, Arc<Mutex<dyn Read + Send + Sync>>>,
    /// Output socket handle
    node_outputs: HashMap<i32, Arc<Mutex<dyn Write + Send + Sync>>>,
    /// Current loading routes
    node_routing: HashMap<i32, Vec<i32>>,
}

impl ServerState {

    pub fn new() -> ServerState {
        ServerState {
            can_broadcast: false,
            node_allocator: 0,
            node_names: HashMap::new(),
            node_inputs: HashMap::new(),
            node_outputs: HashMap::new(),
            node_routing: HashMap::new(),
        }
    }

    fn get_can_broadcast(&self) -> bool {self.can_broadcast}

    fn set_can_broadcast(&mut self, broadcast: bool) {
        self.can_broadcast = broadcast;
    }

    fn get_node_names(&self) -> &HashMap<i32, String> {
        &self.node_names
    }

    fn delete_node(&mut self, node: i32) {
        if node + 1 == self.node_allocator {
            self.node_allocator -= 1;
        }

        self.node_inputs.remove(&node);
        self.node_names.remove(&node);
        self.node_outputs.remove(&node);
        self.node_routing.remove(&node);
    }

    fn new_node_raw<T>(&mut self, nameOp: Option<T>, isOp: Option<Arc<Mutex<dyn Read + Send + Sync>>>, osOp: Option<Arc<Mutex<dyn Write + Send + Sync>>>) -> i32 where T: Into<String> {
        let n = self.node_allocator;
        self.node_allocator += 1;
        if let Some(is) = isOp {
            self.node_inputs.insert(n, is);
        };

        match nameOp {
            Some(name) => self.node_names.insert(n, name.into()),
            None => self.node_names.insert(n, n.to_string()),
        };

        if let Some(os) = osOp {
            self.node_outputs.insert(n, os);
        }

        self.node_routing.insert(n, vec![]);

        n
    }

    fn new_node(&mut self, is: Arc<Mutex<dyn Read + Send + Sync>>, os: Arc<Mutex<dyn Write + Send + Sync>>) -> i32 {
        self.new_node_raw::<String>(None, Some(is), Some(os))
    }

    fn new_sink(&mut self, os: Arc<Mutex<dyn Write + Send + Sync>>) -> i32 {
        self.new_node_raw::<String>(None, None, Some(os))
    }

    fn rename_node(&mut self, node: i32, name: impl Into<String>) -> bool {
        if self.node_names.contains_key(&node) {
            self.node_names.insert(node, name.into());
            true
        } else {
            false
        }
    }

    fn add_route(&mut self, node: i32, target: i32) {
        match self.node_routing.get_mut(&node) {
            Some(vec) => if !vec.contains(&target) { vec.push(target); } ,
            None => (),
        }
    }

    fn del_route(&mut self, node: i32, target: i32) {
        match self.node_routing.get_mut(&node) {
            Some(vec) => vec.retain(|&x| x != target),
            None => (),
        }
    }
}

/// Start listening for tcp socket connections (async)
pub fn run_tcp_listener(server_state: Arc<RwLock<ServerState>>, path: impl Into<String>) -> JoinHandle<Result<()>> {
    let path = path.into();

    thread::spawn(move|| {
        let listener = TcpListener::bind(path).expect("Can't bind tcp port");
        info!("Server is now listening for connections on TCP socket.");
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let state_ref = server_state.clone();
                    thread::spawn(move|| {
                        let mut sock_copy = stream.try_clone().unwrap();
                        let mut input = CodedInputStream::new(&mut sock_copy);
                        run_client(state_ref, stream, &mut input)
                    });
                }
                Err(err) => { error!("{}", err); }
            }
        }
        Ok(())
    })
}

/// Start listening for unix socket connections (async)
pub fn run_unix_listener(server_state: Arc<RwLock<ServerState>>, path: impl Into<String>) -> JoinHandle<Result<()>> {
    let path = path.into();

    thread::spawn(move|| {
        let listener = UnixListener::bind(path).expect("Can't bind unix socket port");
        info!("Server is now listening for connections on Unix Domain socket");
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let state_ref = server_state.clone();
                    thread::spawn(move|| {
                        let mut sock_copy = stream.try_clone().unwrap();
                        let mut input = CodedInputStream::new(&mut sock_copy);
                        run_client(state_ref, stream, &mut input)
                    });
                }
                Err(err) => { error!("{}", err); }
            }
        }
        Ok(())
    })
}

/// Arbitrary client connection manager
fn run_client<T>(server_state: Arc<RwLock<ServerState>>, socket: T, inputStream: &mut CodedInputStream) where T: 'static+Read+Write+Sync+Send {
    debug!("New client connection accepted");
    // Initialize node if needed
    let mut socket_arc = Arc::new(Mutex::new(socket));
    let mut node = None;

    if let Ok(connection) = inputStream.read_message::<rics::RICS_Connection>() {
        if connection.get_connect_as_node() {
            let mut state = server_state.write().unwrap();
            let nd = state.new_node(socket_arc.clone(), socket_arc.clone());
            node = Some(nd);
            debug!("Creating node id {}", nd);
        } else {
            debug!("Opening info connection");
        }
    }

    // Message managing loop
    loop {
        match inputStream.read_message::<rics::RICS_Request>() {
            Ok(req) => {
                info!("Server received message {:?}", req);

                if req.has_set_name() {
                    let mut state = server_state.write().unwrap();
                    node.map(|x| state.rename_node(x, req.get_set_name()));
                    ()
                } else if req.has_query() {
                    match req.get_query() {
                        rics::RICS_Request_RICS_Query::NULL => (),
                        rics::RICS_Request_RICS_Query::LIST_SINK => {

                            let state = server_state.read().unwrap();

                            let mut response = rics::RICS_Response::new();
                            let mut idlist = rics::RICS_Response_RICS_IdList::new();
                            let ids: Vec<_> = state.get_node_names().iter().map(|(k,v)| {
                                let mut id = rics::RICS_Response_RICS_Id::new();
                                id.set_id(*k);
                                id.set_name(v.clone());
                                id
                            }).collect();
                            idlist.set_ids(protobuf::RepeatedField::from_vec(ids));

                            let mut writer = socket_arc.lock().unwrap();
                            response.write_length_delimited_to_writer(&mut *writer);
                        },
                        rics::RICS_Request_RICS_Query::WHO_AM_I => {
                            debug!("Answer WHO_AM_I request with {:?}", node);
                            let mut msg = rics::RICS_Response::new();
                            node.map(|n| msg.set_node(n));
                            let mut writer = socket_arc.lock().unwrap();
                            msg.write_length_delimited_to_writer(&mut *writer);
                        },
                        rics::RICS_Request_RICS_Query::SET_FLAG_CAN_BROADCAST =>
                            server_state.write().unwrap().set_can_broadcast(true),
                        rics::RICS_Request_RICS_Query::CLEAR_FLAG_CAN_BROADCAST =>
                            server_state.write().unwrap().set_can_broadcast(false),
                        rics::RICS_Request_RICS_Query::DAEMON_QUIT => process::exit(2),
                    }
                    ()
                } else if req.has_data() {
                    // Packet message, must forward
                    let state = server_state.read().unwrap();

                    let mut msg = rics::RICS_Response::new();
                    let mut data = req.get_data().clone();
                    if let Some(n) = node { data.set_source(n); }
                    msg.set_data(data.clone());

                    if state.get_can_broadcast() && data.get_field_type() == rics::RICS_Data_RICS_DataType::CAN {
                        // CAN broadcast forwarding
                        for (n, writer) in state.node_outputs.iter() {
                            if Some(*n) != node {
                                msg.write_length_delimited_to_writer(&mut *(writer.lock().unwrap()));
                            }
                        }
                    } else {
                        // Routing forwarding
                        for target in if data.has_target() {
                            vec![data.get_target()]
                        } else {
                            node.and_then(|n| state.node_routing.get(&n).cloned()).unwrap_or(vec![])
                        } {
                            if let Some(writer) = state.node_outputs.get(&target) {
                                info!("Forwarding to {}", target);
                                msg.write_length_delimited_to_writer(&mut *(writer.lock().unwrap()));
                            }
                        }
                        ()
                    }
                } else if req.has_add_route() {
                    let mut state = server_state.write().unwrap();

                    let i = req.get_add_route().get_from();
                    let j = req.get_add_route().get_to();

                    state.add_route(i, j);
                } else if req.has_del_route() {
                    let mut state = server_state.write().unwrap();

                    let i = req.get_del_route().get_from();
                    let j = req.get_del_route().get_to();

                    state.del_route(i, j);
                } else {
                    warn!("Invalid message {:?}", req);
                }
            },
            Err(err) => { warn!("Invalid message query {}, closing connection", err); break; },
        }
    }

    if let Some(n) = node {
        let mut state = server_state.write().unwrap();
        state.delete_node(n);
    }
}

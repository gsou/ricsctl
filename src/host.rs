//! Server hosting features

//use std::fs::{File, remove_file};
use std::io::{Result, Read, Write};
//use std::io::{stdout, stdin};
#[cfg(target_family="unix")]
use std::os::unix::net::{UnixListener};
use std::net::TcpListener;
use std::sync::{RwLock, Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::process;

use protobuf::{Message, CodedInputStream};

use std::collections::{HashMap};
use super::rics;
use rand;
use rand::Rng;


/// Contains server permanent state
pub struct ServerState {
    /// Flag for if the CAN broadcasting is enabled
    can_broadcast: bool,
    /// Determines the likelyhood of a CAN message being dropped
    /// in CAN mode, if a message is dropped, no one receives the
    /// message
    can_drop_chance: f32,
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
            can_drop_chance: 0.00,
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

    fn new_node_raw<T>(&mut self, name_op: Option<T>, is_op: Option<Arc<Mutex<dyn Read + Send + Sync>>>, os_op: Option<Arc<Mutex<dyn Write + Send + Sync>>>) -> i32 where T: Into<String> {
        let n = self.node_allocator;
        self.node_allocator += 1;
        if let Some(is) = is_op {
            self.node_inputs.insert(n, is);
        };

        match name_op {
            Some(name) => self.node_names.insert(n, name.into()),
            None => self.node_names.insert(n, n.to_string()),
        };

        if let Some(os) = os_op {
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

    fn set_node_name(&mut self, node: i32, name: impl Into<String>) {
        let str = name.into();
        trace!("Setting node {} to name {}", node, str.clone());
        self.node_names.insert(node, str);
    }

    fn rename_node(&mut self, node: i32, name: impl Into<String>) -> bool {
        if self.node_names.contains_key(&node) {
            self.set_node_name(node, name);
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

    fn set_can_drop_chance(&mut self, v: f32) {
        if v >= 0.0 && v <= 1.0 {
            self.can_drop_chance = v;
            info!("Changing CAN drop rate to {}", v);
        } else {
            warn!("Invalid CAN drop value: {}", v);
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
#[cfg(target_family="unix")]
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

#[cfg(target_family="windows")]
pub fn run_unix_listener(server_state: Arc<RwLock<ServerState>>, path: impl Into<String>) -> JoinHandle<Result<()>> {
    panic!("Windows is not compatible with Unix domain sockets");
}

/// Arbitrary client connection manager
fn run_client<T>(server_state: Arc<RwLock<ServerState>>, socket: T, input_stream: &mut CodedInputStream) where T: 'static+Read+Write+Sync+Send {
    debug!("New client connection accepted");
    // Initialize node if needed
    let socket_arc = Arc::new(Mutex::new(socket));
    let mut node = None;
    let mut rng = rand::thread_rng();

    if let Ok(connection) = input_stream.read_message::<rics::RICS_Connection>() {
        if connection.get_connect_as_node() {
            let mut state = server_state.write().unwrap();
            let nd = state.new_node(socket_arc.clone(), socket_arc.clone());
            state.set_node_name(nd, format!("{}",nd)); // Default name
            node = Some(nd);
            debug!("Creating node id {}", nd);
        } else {
            debug!("Opening info connection");
        }
    }

    // Message managing loop
    loop {
        match input_stream.read_message::<rics::RICS_Request>() {
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
                                trace!("Reply node pair: {} - {}", *k, v.clone());
                                id
                            }).collect();
                            idlist.set_ids(protobuf::RepeatedField::from_vec(ids));
                            response.set_idlist(idlist);

                            let mut writer = socket_arc.lock().unwrap();
                            response.write_length_delimited_to_writer(&mut *writer).expect("Socket error");
                        },
                        rics::RICS_Request_RICS_Query::WHO_AM_I => {
                            debug!("Answer WHO_AM_I request with {:?}", node);
                            let mut msg = rics::RICS_Response::new();
                            node.map(|n| msg.set_node(n));
                            let mut writer = socket_arc.lock().unwrap();
                            msg.write_length_delimited_to_writer(&mut *writer).expect("Socket error");
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

                    // Broadcast Dropping
                    if state.can_drop_chance != 0.0 && data.get_field_type() == rics::RICS_Data_RICS_DataType::CAN {
                        if rng.gen::<f32>() < state.can_drop_chance {
                            info!("Server is dropping packet {:?}", data);
                            continue;
                        }
                    }
                    

                    // Forwarding
                    if state.get_can_broadcast() && data.get_field_type() == rics::RICS_Data_RICS_DataType::CAN {
                        // CAN broadcast forwarding
                        for (n, writer) in state.node_outputs.iter() {
                            if Some(*n) != node {
                                msg.write_length_delimited_to_writer(&mut *(writer.lock().unwrap())).expect("Socket error");
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
                                msg.write_length_delimited_to_writer(&mut *(writer.lock().unwrap())).expect("Socket error");
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
                } else if req.has_can_drop_chance() {
                    let mut state = server_state.write().unwrap();
                    state.set_can_drop_chance( req.get_can_drop_chance() );
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

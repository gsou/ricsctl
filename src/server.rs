//! Server interaction functions

use std::io::{Read, Write};
use std::collections::HashMap;
use protobuf::{Message, CodedInputStream};
#[cfg(target_family="unix")]
use std::os::unix::net::{UnixStream};
use std::net::TcpStream;
use std::time::Duration;
use std::sync::mpsc::{channel, Receiver};
use super::rics;

#[cfg(target_family="windows")]
type UnixStream = TcpStream;

pub struct RICSServer {
    input: Box<dyn Read + Sync + Send>,
    socket: Box<dyn Write + Sync + Send>,
    //input: CodedInputStream<'a>,
    //output: CodedOutputStream<'a>,
    node_names: HashMap<i32, String>,
    node: i32,
}

pub trait NodeName {
    fn get_name(&self, svr: &mut RICSServer) -> Option<i32> {
        svr.list_nodes();
        self.get_name_cached(svr)
    }

    fn get_name_cached(&self, svr: &RICSServer) -> Option<i32>;
}

impl NodeName for String {
    fn get_name_cached(&self, svr: &RICSServer) -> Option<i32> {
        svr.node_from_name_cached(self.as_str()).get(0).cloned()
    }
}

impl NodeName for i32 {
    fn get_name(&self, _svr: &mut RICSServer) -> Option<i32> {
        Some(*self)
    }

    fn get_name_cached(&self, _svr: &RICSServer) -> Option<i32> {
        Some(*self)
    }
}

pub enum ConnectTo {
    Default,
    Unix(String),
    Tcp(String),
}

impl RICSServer {

    /// Returns the default Unix domain connection type
    #[cfg(target_family="unix")]
    pub fn default_socket() -> UnixStream {
       let socket = UnixStream::connect("/tmp/rics.socket").expect("Failed to connect to server");
       socket.set_read_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
       socket.set_write_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
       socket
    }

    /// Returns the default Unix domain connection type
    #[cfg(target_family="windows")]
    pub fn default_socket() -> TcpStream {
        let socket = TcpStream::connect("localhost:7899").expect("Failed to connect to server");
        socket.set_read_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
        socket.set_write_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
        socket
    }

    /// New server connection using default settings
    pub fn new() -> RICSServer {
       let socket = RICSServer::default_socket();

       RICSServer {
           input: Box::new(socket.try_clone().unwrap()),
           socket: Box::new(socket),
           node_names: HashMap::new(),
           node: 0,
       }
    }

    /// CPS server creation
    pub fn with_server<T, F>(ct: ConnectTo, f: F) -> T
    where F: FnOnce(RICSServer) -> T {
        // Build server
        let server = match ct {
            ConnectTo::Default => RICSServer::new(),
            ConnectTo::Unix(path) => {
                if cfg!(target_family="unix") {
                    let socket = UnixStream::connect(path).expect("Failed to connect to server");
                    socket.set_read_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
                    socket.set_write_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
                    RICSServer::new_from(socket.try_clone().unwrap(), socket)
                } else {
                    panic!("Unix sockets are not supported on Windows")
                }
            },
            ConnectTo::Tcp(path) => {
                let socket = TcpStream::connect(path).expect("Failed to connect to server");
                socket.set_read_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
                socket.set_write_timeout(Some(Duration::new(1,0))).expect("Can't change socket param");
                RICSServer::new_from(socket.try_clone().unwrap(), socket)
            }
        };

        f(server)
    }

    /// New server connection using custom sockets
    pub fn new_from(a: impl 'static + Read + Send + Sync, b: impl 'static + Write + Sync + Send) -> RICSServer {
        RICSServer {
            input: Box::new(a),
            socket: Box::new(b),
            node_names: HashMap::new(),
            node: 0,
        }
    }

    /// Create a connection to the socket using the default rics daemon socket location.
    pub fn connect(&mut self, as_node: bool) {
        debug!("Connecting..");
        let mut msg = rics::RICS_Connection::new();
        msg.set_connect_as_node(as_node);
        msg.write_length_delimited_to_writer(&mut self.socket).expect("Connection message fail");
        trace!("Connection message sent");
    }

    /// Get the current connection id. Also sets the internal number variable.
    pub fn who_am_i(&mut self) ->i32 {
        debug!("Sending WHO_AM_I query");
        let mut msg = rics::RICS_Request::new();
        msg.set_query(rics::RICS_Request_RICS_Query::WHO_AM_I);
        msg.write_length_delimited_to_writer(&mut self.socket).expect("WHO_AM_I query message fail");

        let mut input = CodedInputStream::new(&mut self.input);

        trace!("Waiting for server response");
        self.node = match input.read_message::<rics::RICS_Response>() {
            Ok(resp) => resp.get_node(),
            Err(err) => { warn!("WHO_AM_I bas response: {}", err); 0 },
        };
        trace!("Done processing response");
        self.node
    }

    /// Sets the CAN drop rate for the server
    /// A value of 0 means all messages are forwarded,
    /// while a value of 1 means that no messages are.
    pub fn set_can_drop_chance(&mut self, v: f32) {
        let mut msg = rics::RICS_Request::new();
        msg.set_can_drop_chance(v);
        msg.write_length_delimited_to_writer(&mut self.socket).expect("SET_CAN_DROP_CHANCE fail");
    }

    /// Sets the CAN broadcast flag for the server
    /// A CAN broadcast active means that when a data packet
    /// has CAN type, it will be send to every node regardless
    /// of routing.
    ///
    /// The server does not send a confirmation.
    pub fn set_can_broadcast(&mut self, v: bool) {
        
        debug!("Changing can broadcast flag to {}", v);

        let mut msg = rics::RICS_Request::new();
        msg.set_query(if v { rics::RICS_Request_RICS_Query::SET_FLAG_CAN_BROADCAST } else { rics::RICS_Request_RICS_Query::CLEAR_FLAG_CAN_BROADCAST });
        msg.write_length_delimited_to_writer(&mut self.socket).expect("SET/CLEAR_FLAG_CAN_BROADCAST fail");
    }


    /// Return the currently loaded nodes and their alias
    pub fn list_nodes(&mut self) -> &HashMap<i32, String> {
        debug!("Sending LIST_SINK query");
        let mut msg = rics::RICS_Request::new();
        msg.set_query(rics::RICS_Request_RICS_Query::LIST_SINK);
        msg.write_length_delimited_to_writer(&mut self.socket).expect("LIST_SINK query message fail");

        let mut input = CodedInputStream::new(&mut self.input);

        trace!("Waiting for server response");
        self.node_names = match input.read_message::<rics::RICS_Response>() {
            Ok(resp) => resp.get_idlist().get_ids().iter().map(|id| (id.get_id(),id.get_name().into())).collect(),
            Err(err) => { warn!("LIST_SINK bad response: {}", err); HashMap::new() },
        };
        trace!("Done processing response, got {:?}", self.node_names);
        &self.node_names
    }

    pub fn node_from_string_cached(&self, str: impl Into<String>) -> Option<i32> {
        let str = str.into();
        str.parse::<i32>().ok().and_then(|n| n.get_name_cached(self)).or_else(|| str.get_name_cached(self))
    }

       
    pub fn node_from_string(&mut self, str: impl Into<String>) -> Option<i32> {
        let str = str.into();
        str.parse::<i32>().ok().and_then(|n| n.get_name(self)).or_else(|| str.get_name(self))
    }

    /// Get the node id from a name
    pub fn node_from_name_cached(&self, name: impl Into<String>) -> Vec<i32> {
        let mut v = vec![];
        let name = name.into();
        for (i, n) in &self.node_names {
            if n == &name {
                v.push(*i);
            }
        }
        v
    }

    /// Get the node id from a name
    pub fn node_from_name(&mut self, name: impl Into<String>) -> Vec<i32> {
        self.list_nodes();
        self.node_from_name_cached(name)
    }

    /// High-level delete route between two nodes
    pub fn del_route(&mut self, from: impl NodeName, to: impl NodeName) -> bool{
        if let Some(f) = from.get_name(self) {
            if let Some(t) = to.get_name_cached(self) {
                let mut route = rics::RICS_Route::new();
                route.set_from(f);
                route.set_to(t);

                let mut req = rics::RICS_Request::new();
                req.set_del_route(route);

                self.send_request(req);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// High-level add route between two nodes
    pub fn add_route(&mut self, from: impl NodeName, to: impl NodeName) -> bool{
        if let Some(f) = from.get_name(self) {
            if let Some(t) = to.get_name_cached(self) {
                let mut route = rics::RICS_Route::new();
                route.set_from(f);
                route.set_to(t);

                let mut req = rics::RICS_Request::new();
                req.set_add_route(route);

                self.send_request(req);
                true
            } else {
                false
            }
        } else {
            false
        }
    }


    /// Send a request to the server
    pub fn send_request(&mut self, msg: rics::RICS_Request) {
        debug!("Request sent as: {:?}", &msg);
        msg.write_length_delimited_to_writer(&mut self.socket).expect("Fail to send packet");
    }

    /// Send a generic RICS data packet to a specific node
    pub fn send_packet_to(&mut self, data: rics::RICS_Data, target: i32) {
        let mut msg = rics::RICS_Request::new();
        let mut data = data;
        debug!("Data sent: {:?}; To: {}", &data, target);
        data.set_target(target);
        msg.set_data(data);
        self.send_request(msg)
    }

    /// Send a generic RICS data packet to the default route
    pub fn send_packet(&mut self, data: rics::RICS_Data) {
        let mut msg = rics::RICS_Request::new();
        let mut data = data;
        data.clear_target();
        debug!("Data sent: {:?}", &data);
        msg.set_data(data);
        self.send_request(msg)
    }

    /// Blocks and wait for the next server message
    pub fn get_response(&mut self) -> Option<rics::RICS_Response> {
        debug!("Getting packet...");

        let mut input = CodedInputStream::new(&mut self.input);

        match input.read_message::<rics::RICS_Response>() {
            Ok(resp) => Some(resp),
            Err(err) => { warn!("GET_PACKET bad response: {}", err); None},
        }
    }

    /// Read packets to a channel. The packets can't be read anymore elsewhere
    pub fn listen_response(&mut self) -> Receiver<rics::RICS_Response> {
        
        let (tx, rx) = channel();

        let mut input_reader: Box<dyn Read + Sync + Send> = Box::new(std::io::empty());
        std::mem::swap(&mut input_reader, &mut self.input);

        std::thread::spawn(move|| {
            let mut input = CodedInputStream::new(&mut input_reader);
            loop {
                if let Ok(resp) = input.read_message::<rics::RICS_Response>() {
                    tx.send(resp).expect("Brocken listen_response channel");
                }
            }
        });

        rx
    }

    /// Blocks and wait for the next packet
    pub fn get_packet(&mut self) -> Option<rics::RICS_Data> {
        debug!("Getting packet...");

        let mut input = CodedInputStream::new(&mut self.input);

        match input.read_message::<rics::RICS_Response>() {
            Ok(resp) => if resp.has_data() {
                let data = resp.get_data();
                Some(data.clone())
            } else { None },
            Err(err) => { warn!("GET_PACKET bad response: {}", err); None},
        }
    }

}


pub fn can_packet(id: i32, dat: Vec<u8>) -> rics::RICS_Data {
    let mut data = rics::RICS_Data::new();
    data.set_id(id);
    data.set_data(dat);
    data.set_field_type(rics::RICS_Data_RICS_DataType::CAN);
    data
}

pub fn stream_packet(dat: Vec<u8>) -> rics::RICS_Data {
    let mut data = rics::RICS_Data::new();
    data.set_data(dat);
    data.set_field_type(rics::RICS_Data_RICS_DataType::STREAM);
    data
}


pub fn data_to_string(data: &rics::RICS_Data) -> String {
    format!("<{} -> {} ({}) [{}]>", data.get_source(),
            data.get_target(),
            data.get_id(),
            data.get_data().iter().map(|x| x.to_string())
            .collect::<Vec<String>>().join(", "))
}

pub fn response_to_string(resp: &rics::RICS_Response) -> String {
    if resp.has_node() {
        format!("<WHO_AM_I: {}>", resp.get_node())
    } else if resp.has_data() {
        data_to_string(resp.get_data())
    } else {
        format!("<???>")
    }
}

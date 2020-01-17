extern crate clap;
extern crate env_logger;
extern crate protobuf;
#[macro_use] extern crate log;
extern crate serialport;
extern crate libloading;
extern crate libc;
extern crate rlua;
extern crate rand;

mod script;
mod server;
mod rics;
mod host;
use host::ServerState;

use std::fs::File;
use std::thread;
use std::io::{stdout, Read, Write};
#[cfg(target_family="unix")]
use std::os::unix::io::FromRawFd;
use std::sync::{Arc, RwLock, Mutex};
use clap::{Arg, App, SubCommand};

#[derive(Debug)]
struct Packet {
    id: i32,
    dim: u8,
    dat: [u8; 8]
}


fn main() {
    env_logger::init();

    // Argument parsing
    let matches = App::new("ricsctl")
        .version("0.1.0")
        .author("Skami")
        .about("Control a RICS server")
        .arg(Arg::with_name("unix_client")
             .short("u")
             .long("uds")
             .help("If a client or server is started, it will connect to the unix domain socket located at the given path")
             .number_of_values(1)
             .multiple(true)
             .required(false)
             .takes_value(true))
        .arg(Arg::with_name("tcp")
             .short("t")
             .long("tcp")
             .number_of_values(1)
             .multiple(true)
             .required(false)
             .takes_value(true)
             .help("If a client or server is started, it will connect to the given tcp socket"))
        .subcommand(SubCommand::with_name("plugin")
                    .about("Load an external processing plugin")
                    .arg(Arg::with_name("lua")
                         .long("lua")
                         .number_of_values(1)
                         .required_unless("dynlib")
                         .help("Lua scripting file for advanced operation")
                         .multiple(false)
                         .takes_value(true))
                    .arg(Arg::with_name("dynlib")
                         .short("d")
                         .long("dynlib")
                         .required_unless("lua")
                         .number_of_values(1)
                         .conflicts_with("lua")
                         .help("Dynamic library plugin for advanced processing on clients")
                         .takes_value(true)))
        .subcommand(SubCommand::with_name("list")
                    .about("List available nodes and their names")) // TODO
        .subcommand(SubCommand::with_name("start")
                    .about("Start a server with given connections"))
        .subcommand(SubCommand::with_name("stop")
                    .about("Close the RICS server")) // TODO
        .subcommand(SubCommand::with_name("stream")
                    .about("Transit through stdout and stdin")
                    .arg(Arg::with_name("source_stream")
                         .short("i")
                         .long("source")
                         .takes_value(false)
                         .required(false)
                         .help("Do not receive messages"))
                    .arg(Arg::with_name("sink_stream")
                         .short("o")
                         .long("sink")
                         .takes_value(false)
                         .required(false)
                         .help("Do not send messages")))
        .subcommand(SubCommand::with_name("log")
                    .about("Logs every received packet"))
        .subcommand(SubCommand::with_name("route")
                    .about("Change sever routing")
                    .arg(Arg::with_name("SRC")
                         .required(true)
                         .index(1)
                         .help("Source node"))
                    .arg(Arg::with_name("del")
                         .takes_value(false)
                         .required(false)
                         .short("d")
                         .long("delete")
                         .help("Delete routes instead of adding them"))
                    .arg(Arg::with_name("to")
                         .multiple(true)
                         .takes_value(true)
                         .required(false)
                         .short("t")
                         .long("to")
                    .help("Destination nodes to add")))
        .subcommand(SubCommand::with_name("can")
                    .about("Internal can protocol")
                    .subcommand(SubCommand::with_name("broadcast")
                                .about("Set server's can broadcast flag")
                                .arg(Arg::with_name("BROADCAST")
                                     .index(1)
                                     .required(true)))
                    .subcommand(SubCommand::with_name("drop")
                                .about("Set server's can chance of dropping a CAN message")
                                .arg(Arg::with_name("DROP")
                                     .index(1)
                                     .help("A lua expression representing a floating point number between 0 and 1")
                                     .required(true)))
                    .subcommand(SubCommand::with_name("send")
                                .about("Send a can message")
                                .arg(Arg::with_name("id")
                                     .short("i")
                                     .long("id")
                                     .takes_value(true)
                                     .required(true)
                                     .help("CAN id"))
                                .arg(Arg::with_name("data")
                                     .short("d")
                                     .long("data")
                                     .takes_value(true)
                                     .required(true)
                                     .help("CAN message content"))
                                .arg(Arg::with_name("target")
                                     .short("t")
                                     .long("target")
                                     .required(false)
                                     .takes_value(true)
                                     .help("Node target for the message")))
                    .subcommand(SubCommand::with_name("log")
                                .about("Log CAN messages"))
                    .subcommand(SubCommand::with_name("serial")
                                .about("Push serial CAN messages to the stream and back")
                                .arg(Arg::with_name("PORT")
                                     .help("The serial port")
                                     .required(true)
                                     .index(1))
                                .arg(Arg::with_name("BAUD")
                                     .help("Baud rate at which to operate")
                                     .required(false)
                                     .index(2))
                                .arg(Arg::with_name("target")
                                     .short("t")
                                     .long("target")
                                     .takes_value(true)
                                     .help("Node target for the messages")))
        ) .get_matches();


    // Get server paths
    let mut unix_domain_servers: Vec<&str> = matches.values_of("unix_client").unwrap_or_default().collect();
    let mut tcp_servers: Vec<&str> = matches.values_of("tcp").unwrap_or_default().collect();
    if tcp_servers.is_empty() && unix_domain_servers.is_empty() {
        if cfg!(target_family="unix") {
            info!("Server path not provided, using default /tmp/rics.socket");
            unix_domain_servers.push("/tmp/rics.socket");
        } else {
            info!("Server path not provided, using default localhost:7299");
            tcp_servers.push("localhost:7299");
        }
    }

    if let Some(_matches) = matches.subcommand_matches("start") {
        ////////////////////// SERVER MODE //////////////////////
        info!("Starting server...");
        let server_state: Arc<RwLock<ServerState>> = Arc::new(RwLock::new(ServerState::new()));

        // Starting connecton points
        for unix in unix_domain_servers {
            info!("Launching Unix Domain listener on {}", unix.clone());
            host::run_unix_listener(server_state.clone(), unix);
        }

        // Starting connection points
        for tcp in tcp_servers {
            info!("Launching TCP listener on {}", tcp.clone());
            host::run_tcp_listener(server_state.clone(), tcp);
        }

        info!("...Server started");
        loop {}

    } else {
        ////////////////////// CLIENT MODE //////////////////////

        let conn = unix_domain_servers.get(0).map(|x| server::ConnectTo::Unix((*x).to_string()))
            .or(tcp_servers.get(0).map(|x| server::ConnectTo::Tcp((*x).to_string())))
            .unwrap_or(server::ConnectTo::Default);

        server::RICSServer::with_server(conn, move|mut svr| {

            ///////////////////// PLUGIN ENGINE /////////////////////////
            if let Some(matches) = matches.subcommand_matches("plugin") {
                trace!("Loading plugin engine...");
                // Load plugin engine
                let engine: Box<dyn script::ScriptingInterface> = if matches.is_present("dynlib") {
                    Box::new(script::DynlibScript::new(matches.value_of("dynlib").unwrap().to_string()).load())
                } else if matches.is_present("lua") {
                    Box::new(script::LuaScript::new(matches.value_of("lua").unwrap().to_string()))
                } else {
                    Box::new(script::NoEngine)
                };

                trace!("Initializing plugin engine...");
                if !engine.initialize() {
                    error!("Plugin engine initialization failed");
                }

                trace!("Connecting to server...");
                svr.connect(true);
                svr.list_nodes();

                let node = svr.who_am_i();
                info!("Connecting on node id {}", node);

                trace!("Starting plugin engine...");
                if !engine.start(&mut svr, node) {
                    error!("Plugin engine start failed");
                }

                let rx = svr.listen_response();

                let mut update_enabled = true;
                loop {
                    if let Ok(resp) = rx.try_recv() {
                        if resp.has_data() {
                            let p = resp.get_data();
                            if p.get_field_type() == rics::RICS_Data_RICS_DataType::CAN {
                                info!("Sending can message {} to plugin", p.get_id());
                                engine.can_rx(&mut svr, p.get_id() as u32, p.get_data().to_vec());
                            }

                            update_enabled = true
                        }
                    } else if update_enabled {
                        update_enabled = engine.update(&mut svr);
                    }
                }

            } else if let Some(matches) = matches.subcommand_matches("route") {
                /////////////////////// ROUTING //////////////////////////
                svr.connect(false);
                svr.list_nodes();
                let source = svr.node_from_string_cached(matches.value_of("SRC").unwrap()).expect("Invalid source node number");
                let dests: Vec<i32> = matches.values_of("to").unwrap().map(|x| svr.node_from_string_cached(x).expect("Invalid destination node")).collect();
                let delete = matches.is_present("del");
                for d in dests {
                    if delete {svr.del_route(source, d);} else {svr.add_route(source, d);}
                }
            }
            else if let Some(matches) = matches.subcommand_matches("can") {
                if let Some(matches) = matches.subcommand_matches("broadcast") {
                    //////////////////////// CAN BROADCAST FLAG ///////////////////
                    svr.connect(false);
                    svr.set_can_broadcast(rlua::Lua::new().context(|ctx| { match ctx.load(&matches.value_of("BROADCAST").unwrap()).eval() {
                        Ok(flag) => flag,
                        Err(e) => { error!("Invalid format for bool flag: {}", e); std::process::exit(1)}
                    }}));
                }
                else if let Some(matches) = matches.subcommand_matches("drop") {
                    //////////////////////// CAN DROP CHANCE /////////////////
                    svr.connect(false);
                    svr.set_can_drop_chance(rlua::Lua::new().context(|ctx| { match ctx.load(&matches.value_of("DROP").unwrap()).eval() {
                        Ok(f) => f,
                        Err(e) => { error!("Invalid format for float DROP: {}", e); std::process::exit(1)}
                    }}));
                }
                else if let Some(matches) = matches.subcommand_matches("send") {
                    //////////////////////// CAN SEND FLAG ///////////////////
                    svr.connect(false);

                    let (id, data) = rlua::Lua::new().context(|ctx| {
                        let id: i32 = match ctx.load(&matches.value_of("id").unwrap()).eval() {
                            Ok(id) => id,
                            Err(e) => {
                                error!("Invalid format for CAN id: {}", e);
                                std::process::exit(1)
                            }
                        };
                        let data: Vec<u8> = match ctx.load(&matches.value_of("data").unwrap()).eval() {
                            Ok(id) => id,
                            Err(e) => {
                                error!("Invalid format for CAN data: {}", e);
                                std::process::exit(1)
                            }
                        };
                        (id,data)
                    });

                    if matches.is_present("target") {
                        let target = matches.value_of("target").unwrap().parse::<i32>().expect("Invalid target number");
                        svr.send_packet_to(server::can_packet(id, data), target);
                    } else {
                        svr.send_packet(server::can_packet(id, data));
                    }
                }
                else if let Some(_) = matches.subcommand_matches("log") {
                    //////////////////////////////// CAN LOG ///////////////////////////
                    svr.connect(true);
                    let node = svr.who_am_i();
                    println!("Logging on node id {}", node);
                    loop {
                        if let Some(p) = svr.get_packet() {
                            if p.get_field_type() == rics::RICS_Data_RICS_DataType::CAN {
                                println!("{}", server::data_to_string(&p));
                            }
                        }
                    }
                }
                else if let Some(matches) = matches.subcommand_matches("serial") {
                    //////////////////////////// KVASER ///////////////////////////
                    svr.connect(true);
                    let node = svr.who_am_i();
                    println!("Sending on node id {}", node);

                    // Serial port
                    let mut port = serialport::open(matches.value_of("PORT").unwrap()).expect("Unable to open serialport");
                    port.set_baud_rate(matches.value_of("BAUD").unwrap_or("115200").parse::<u32>().expect("Invalid baudrate")).expect("Failed to set port baudrate");
                    let mut v = Vec::new();
                    let target = if matches.is_present("target") {
                        Some( matches.value_of("target").unwrap().parse::<i32>().expect("Invalid target number") )
                    } else { None };
                    // TODO TX
                    loop {
                        let mut buf = [0u8;128];
                        if let Ok(len) = port.read(&mut buf) {
                            v.extend_from_slice(&buf[..len]);
                        }

                        if v.len() >= 13 {
                            v = v.into_iter().skip_while(|x| *x != 0xFA).skip(1).collect();
                        }

                        if v.len() >= 13 {
                            let mess : Packet = unsafe { std::ptr::read(v[..13].as_ptr() as *const _) };
                            debug!("{:?}", mess);
                            v = v[13..].to_vec();
                            let contents = mess.dat.to_vec().into_iter().take(mess.dim as usize).collect();
                            let data = server::can_packet(mess.id, contents);
                            match target {
                                Some(t) => svr.send_packet_to(data, t),
                                None => svr.send_packet(data),
                            }
                        }
                    }
                }
            }
            else if let Some(matches) = matches.subcommand_matches("stream") {
                //////////////////////// STREAM ///////////////////////////
                svr.connect(true);
                let node = svr.who_am_i();
                println!("Sending on node id {}", node);

                let svr_arc = Arc::new(Mutex::new(svr));
                let svr_copy = svr_arc.clone();
                let source_stream = matches.is_present("source_stream");
                let sink_stream = matches.is_present("sink_stream");

                // TODO windows compatibility
                #[cfg(target_family="unix")]
                {
                if cfg!(target_family="unix") {
                    if !sink_stream {
                        thread::spawn(move || {
                            debug!("Starting stdin thread");
                            let mut stdin_handle = unsafe { File::from_raw_fd(0) };
                            loop {
                                let mut buffer = [0u8;2048];
                                let n = stdin_handle.read(&mut buffer[..]).expect("Can't access stdin");
                                if n>0 {
                                    trace!("Sending packet to server");
                                    let mut svr = svr_copy.lock().unwrap();
                                    svr.send_packet(server::stream_packet(buffer[..n].to_vec()));
                                } else {
                                    trace!("No bytes received by stdin read");
                                }
                            }
                        });
                    }
                }
                }

                if !source_stream { 
                    debug!("Starting stdout loop");
                    loop {
                        let packet = svr_arc.lock().unwrap().get_packet();
                        let mut stdout_handle = stdout();
                        debug!("{:?}", packet);
                        if let Some(p) = packet {
                            if p.get_field_type() == rics::RICS_Data_RICS_DataType::STREAM {
                                trace!("Sending packet to stdout");
                                stdout_handle.write_all(p.get_data()).expect("Can't write to stdout");
                                stdout_handle.flush().expect("Can't flust stdout");
                            }
                        }
                    }
                }

                loop {}

            }
            else if let Some(_matches) = matches.subcommand_matches("log") {
                svr.connect(true);
                let node = svr.who_am_i();
                println!("Logging on node id {}", node);
                loop {
                    let packet = svr.get_response();
                    debug!("{:?}", packet);
                    if let Some(p) = packet {
                        println!("{}", server::response_to_string(&p));
                    }
                }
            }
        });
    }
}

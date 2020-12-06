
# Table of Contents

1.  [ricsctl](#org45bdbd6)
    1.  [Installation](#org3d1fcec)
        1.  [Windows](#org7e099cd)
        2.  [Linux](#org91bcddc)
    2.  [Usage](#org6692e9d)
        1.  [Server configuration](#orgced52ff)
        2.  [Server usage](#orgfee989a)
        3.  [Lua API](#org907383a)
        4.  [Dynlib API](#org29f14f5)
        5.  [Rust API](#org936ceca)
        6.  [GUI Interface](#guiiface)



<a id="org45bdbd6"></a>

# ricsctl

ricsctl, or Remote Inter-Connected Streams is a server, client and library that allows the virtualisation of bus and stream connection. Typical usage include adding virtual or simulated modules to a bus such as a CAN bus, or allow easy control and monitoring of busses.


<a id="org3d1fcec"></a>

## Installation


<a id="org7e099cd"></a>

### Windows

The build process on Windows I use is a bit weird, so an installer is provided in the release section. If your usage requires a custom build please open an issue.


<a id="org91bcddc"></a>

### Linux

Requirements:

-   protoc
-   lua 5.4

Installation:

    cargo install --path .


<a id="org6692e9d"></a>

## Usage

RICS uses a server client approach in order to connect multiple processes to each other. Each client can send different types of messages to each other in a configurable routing pattern. Each client is also able to configure the server.

RICS can use two connection systems, Unix Domain Sockets and TCP Sockets. The server can be started to listen on multiple connections from the command line. The default socket are `/tmp/rics.socket` on Linux, and `localhost:7299` on Windows.

To configure the allowed connection points, start the server with the `--tcp` or `--uds` command line arguments.

Examples:

    ricsctl start # Starts the server listening only on the default connection point
    ricsctl --tcp 192.168.1.100:1000 start # Starts the server listening only on the tcp socket 192.168.1.100 port 1000
    ricsctl --uds /tmp/path.socket start # Starts the server listening only on the unix domain docket /tmp/path.socket (must not exist)
    ricsctl --tcp 192.168.1.100:1000 --tcp localhost:80000 --uds uds.socket start # Start the server listening on the three provided locations

Once the server is started, multiple packet types can be sent. The following table shows all the supported packet types. Multiple options can be used to configure the server response to different protocol types

<table border="2" cellspacing="0" cellpadding="6" rules="groups" frame="hsides">


<colgroup>
<col  class="org-left" />

<col  class="org-left" />
</colgroup>
<thead>
<tr>
<th scope="col" class="org-left">Name</th>
<th scope="col" class="org-left">Description</th>
</tr>
</thead>

<tbody>
<tr>
<td class="org-left">RAW</td>
<td class="org-left">Raw packet type, used internally</td>
</tr>


<tr>
<td class="org-left">STREAM</td>
<td class="org-left">Part of a stream-like communication protocol</td>
</tr>


<tr>
<td class="org-left">CAN</td>
<td class="org-left">CAN bus emulation</td>
</tr>


<tr>
<td class="org-left">DATAGRAM</td>
<td class="org-left">Datagram based communication protocol</td>
</tr>


<tr>
<td class="org-left">SYNC</td>
<td class="org-left">Syncing packet for timing critical protocols</td>
</tr>
</tbody>
</table>

The environment variable `RUST_LOG` can be used to configure the verbosity of the output. `RUST_LOG=trace` will show all debug information.


<a id="orgced52ff"></a>

### Server configuration

All the following commands can use the `--tcp` and `--uds` options to select the server to configure.

    ricsctl list

Lists the connected nodes.

    ricsctl stop

Stops the connected server.

    ricsctl route SOURCE -t TARGET1 TARGET2

Connect the node named `SOURCE` to the targets `TARGET1`, `TARGET2`. Add the flag `-d` to disconnect instead. All messages sent by `SOURCE` will be received by `TARGET1` and `TARGET2`, but not the other way around.

    ricsctl can broadcast true/false

Sets the CAN broadcast flag. If the CAN broadcast is set to true, all messages of type CAN will be sent to every other node.


<a id="orgfee989a"></a>

### Server usage

The command line tool also provide some common client operations.

    ricsctl log

Display all received messages.

    ricsctl can connect CANIFACE

Connects the server to the socketcan interface CANIFACE. (Linux only)

    ricsctl can serial PORT [BAUD]

Connect to a serial to CANbus converter. I use this internally, please open a issue if you are interested in using this.

    ricsctl can log

Display all received can messages.

    ricsctl can send --id 12 --data '{0,1,2,3}'

Send a CAN message. The data and id parameters must be valid Lua.

    ricsctl plugin --lua file.lua
    ricsctl plugin --dynlib dynlib.dll/dynlib.so

Provides a easy way to run a user application on the server. The Lua and Dynlib api are described below.


<a id="org907383a"></a>

### Lua API

The lua file must contain the following lua functions:

    function rics_start(svr, node) -- Called on server connection (node is your node number)
      return true
    end
    
    function rics_update(svr) -- Called frequently
      return true
    end
    
    function rics_init() -- Called as soon as the Lua engine is loaded
      return true
    end
    
    function rics_can_callback(svr, id, data) -- Called when a can message is received
      return true
    end


<a id="org29f14f5"></a>

### TODO Dynlib API

Please open an issue if you are interested in using this.


<a id="org936ceca"></a>

### Rust API

A rust library can be used to build user applications. The rust documentation can be built with `cargo doc`.

A simple example of using the RICS library:

    extern crate rics;
    use rics::server;
    
    fn main() {
        server::RICSServer::with_server(server::ConnectTo::Default, move|mut svr| {
          svr.connect(true); // Connect as node
          let node = svr.who_am_i(); // Get node number
          println!("My node name is: {}", node);
          svr.send_packet(server::can_packet(12, vec![node])); // Send the node id on packet
        });
    }


<a id="guiiface"></a>

### GUI Interface

A simple gtk GUI interface is provided to monitor and log messages. It is started with the `gui` subcommand.

It is possible to use a filter that will color the messages and provide the data in a more human readable format. This filter file must be a lua file that contains the following function:

    function filter_can(id, data)
        # id is a u32
        # data is a u8 vector
        # Return a tuple of three elements:
        # Bool: False to ignore the message (drop)
        # string: The human readable data format
        # string: Color name (e.g. "black" or "red")
    end


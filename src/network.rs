use crate::interface::*;
use std::io::{ErrorKind, Read};
use std::net::TcpListener;
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::{fs, io, thread, process};

mod handshake;
mod music_exchange;
pub(crate) mod notification;
mod peer;
mod request;
mod response;

extern crate get_if_addrs;
extern crate rand;

use rand::Rng;

use crate::audio::{
    continue_paused_music, create_sink, pause_current_playing_music, play_music,
    stop_current_playing_music, MusicPlayer,
};

use crate::utils::FileStatus::{DELETE, DOWNLOAD, NEW};
use crate::utils::{AppListener, FileInstructions, HEARTBEAT_SLEEP_DURATION};
use handshake::send_table_request;
use notification::*;
use peer::create_peer;
use request::{
    change_peer_name, delete_file_request, delete_from_network, dropped_peer, exist_file,
    exist_file_response, exit_peer, find_file, get_file, get_file_response, order_song_request,
    push_to_db, redundant_push_to_db, request_for_table, self_status_request, send_network_table,
    send_network_update_table, status_request,
};
use std::collections::HashMap;
use std::path::Path;

fn validate_port(port: &str) -> Result<&str, String> {
    if let Err(_e) = port.parse::<u32>() {
        return Err("The supplied port is not numeric".to_string());
    }
    if port.len() != 4 {
        return Err("The supplied port does not have four digits".to_string());
    }
    Ok(port)
}

#[cfg(target_os = "macos")]
pub fn get_own_ip_address(port: &str) -> Result<SocketAddr, String> {
    let ifs = match get_if_addrs::get_if_addrs() {
        Ok(v) => v,
        Err(_e) => return Err("Failed to find any network address".to_string()),
    };
    let if_options = ifs
        .into_iter()
        .find(|i| i.name == "en0" && i.addr.ip().is_ipv4());
    let this_ipv4: String = if let Some(interface) = if_options {
        interface.addr.ip().to_string()
    } else {
        "Local ip address not found".to_string()
    };
    println!("Local IP Address: {}", this_ipv4);
    if let Err(e) = validate_port(&port) {
        return Err(e);
    }
    let ipv4_port = format!("{}:{}", this_ipv4, port);
    let peer_socket_addr = match ipv4_port.parse::<SocketAddr>() {
        Ok(val) => val,
        Err(_e) => return Err("Could not parse ip address to SocketAddr".to_string()),
    };
    Ok(peer_socket_addr)
}

// This function only gets compiled if the target OS is linux
#[cfg(not(target_os = "macos"))]
pub fn get_own_ip_address(port: &str) -> Result<SocketAddr, String> {
    let this_ipv4 = match local_ipaddress::get() {
        Some(val) => val,
        None => return Err("Failed to find any network address".to_string()),
    };
    println!("Local IP Address: {}", this_ipv4);
    let ipv4_port = format!("{}:{}", this_ipv4, port);
    if let Err(e) = validate_port(&port) {
        return Err(e);
    }
    let peer_socket_addr = match ipv4_port.parse::<SocketAddr>() {
        Ok(val) => val,
        Err(e) => return Err("Could not parse ip address to SocketAddr".to_string()),
    };
    Ok(peer_socket_addr)
}

/// Create or join a network, depending on the value of `ip_address`. If the value is `None`, a new
/// network will be created. Otherwise the library will attempt to join an existing network on that
/// IP address.
/// # Parameters
/// `own_name` - name of the local peer
///
/// `port` - port on which the local peer will listen on
///
/// `ip_address` - IP Address of one of the peers of an existing network / `None` if a new network
/// is to be created
///
/// `app` - listener object of the application that implements the library.
/// # Returns
/// the peer object wrapped in a Mutex
pub fn startup(
    own_name: &str,
    port: &str,
    ip_address: Option<SocketAddr>,
    app_arc: Arc<Mutex<Box<dyn AppListener + Sync>>>,
) -> Result<Arc<Mutex<Peer>>, String> {
    let (sender, receiver): (SyncSender<Notification>, Receiver<Notification>) =
        mpsc::sync_channel(5);
    let sender_clone_peer = sender.clone();
    let peer = match create_peer(own_name, port, sender_clone_peer) {
        Ok(p) => p,
        Err(e) => {
            return Err(e);
        }
    };
    let own_addr = peer.ip_address;

    let peer_arc = Arc::new(Mutex::new(peer));
    let peer_arc_clone_listen = peer_arc.clone();
    let peer_arc_clone_return = peer_arc.clone();
    let peer_arc_clone_working = peer_arc.clone();
    let app_arc_working = app_arc.clone();

    let sink = Arc::new(Mutex::new(match create_sink() {
        Ok(s) => s,
        Err(e) => {
            return Err(e);
        }
    }));
    let _working_thread = thread::Builder::new()
        .name("working_thread".to_string())
        .spawn(move || loop {
            let ele = receiver.recv();
            match ele {
                Ok(not) => {
                    let mut peer = match peer_arc_clone_working.lock() {
                        Ok(p) => p,
                        Err(e) => e.into_inner(),
                    };
                    let mut app = match app_arc_working.lock() {
                        Ok(a) => a,
                        Err(e) => e.into_inner(),
                    };
                    let mut sink = match sink.lock() {
                        Ok(s) => s,
                        Err(e) => e.into_inner(),
                    };
                    handle_notification(not, &mut peer, &mut sink, &mut app);
                }
                Err(e) => {
                    println!("error {}", e);
                }
            }
        });

    if let Err(e) = thread::Builder::new()
        .name("TCPListener".to_string())
        .spawn(move || {
            if let Err(e) = listen_tcp(peer_arc_clone_listen, sender) {
                println!("Failed to create connection: {:?}", e);
                process::exit(1);
            };
        }) {
        println!("{:?}", e);
    };

    let _peer_arc_clone_interact = peer_arc.clone();

    //send request existing network table
    match ip_address {
        Some(ip) => {
            send_table_request(ip, own_addr, own_name);
        }
        None => {
            println!("Ip address is empty");
        }
    }

    if let Err(_e) = thread::Builder::new()
        .name("Heartbeat".to_string())
        .spawn(move || {
            if let Err(e) = start_heartbeat(peer_arc) {
                eprintln!("Failed to spawn heartbeat, {:?}", e);
            }
        })
    {
        return Err("Failed to spawn heartbeat".to_string());
    };

    Ok(peer_arc_clone_return)
}

fn listen_tcp(arc: Arc<Mutex<Peer>>, sender: SyncSender<Notification>) -> Result<(), String> {
    let peer = match arc.lock() {
        Ok(p) => p,
        Err(e) => e.into_inner(),
    };
    let listen_ip = peer.ip_address;
    drop(peer);
    let listener = match TcpListener::bind(&listen_ip) {
        Ok(l) => l,
        Err(_e) => {
            println!("Error: {:?}", _e);
            return Err("Could't bind TCP Listener.".to_string());
        }
    };
    for stream in listener.incoming() {
        let mut buf = String::new();
        match stream {
            Ok(mut s) => {
                if let Err(_e) = s.read_to_string(&mut buf) {
                    error!("Could not read the stream to a string.");
                };
                let des: Notification = match serde_json::from_str(&buf) {
                    Ok(val) => val,
                    Err(e) => {
                        dbg!(e);
                        println!("Could not deserialize notification");
                        continue; // skip this stream
                    }
                };
                if let Err(_e) = sender.send(des) {
                    error!("Could not send notification through the channel.");
                };
            }
            Err(_e) => {
                println!("could not read stream");
                return Err("Error".to_string());
            }
        };
    }
    Ok(())
}

/// starts the heartbeat
fn start_heartbeat(arc: Arc<Mutex<Peer>>) -> Result<(), String> {
    loop {
        thread::sleep(HEARTBEAT_SLEEP_DURATION);
        let peer = match arc.lock() {
            Ok(p) => p,
            Err(e) => e.into_inner(),
        };
        let mut peer_clone = peer.clone();
        drop(peer);
        let network_size = peer_clone.network_table.len();
        if network_size == 1 {
            continue;
        } else if network_size <= 20 {
            send_heartbeat(&peer_clone.get_all_socketaddr_from_peers(), &mut peer_clone);
        } else {
            let successors = peer_clone.get_heartbeat_successors();
            send_heartbeat(&successors, &mut peer_clone);
        }
    }
}

/// send the heartbeat request to all targets in `targets`
fn send_heartbeat(targets: &[SocketAddr], peer: &mut Peer) {
    let mut cloned_peer = peer.clone();
    for addr in targets {
        let stream = match TcpStream::connect(addr) {
            Ok(s) => s,
            Err(_e) => {
                handle_lost_connection(*addr, &mut cloned_peer);
                return;
            }
        };
        let not = Notification {
            content: Content::Heartbeat,
            from: *cloned_peer.get_ip(),
        };
        match serde_json::to_writer(&stream, &not) {
            Ok(ser) => ser,
            Err(_e) => {
                println!("Failed to serialize SendRequest {:?}", &not);
            }
        };
    }
}

fn handle_notification(
    notification: Notification,
    peer: &mut Peer,
    sink: &mut MusicPlayer,
    listener: &mut Box<dyn AppListener + Sync>,
) {
    //dbg!(&notification);
    let sender = notification.from;
    match notification.content {
        Content::PushToDB { key, value, .. } => {
            push_to_db(key, value, peer, listener);
        }
        Content::RedundantPushToDB { key, value, from } => {
            redundant_push_to_db(key, value, peer, listener, from);
        }
        Content::ChangePeerName { value } => {
            change_peer_name(value, sender, peer);
        }
        Content::SendNetworkTable { value } => {
            send_network_table(value, peer);
        }
        Content::SendNetworkUpdateTable { value } => {
            send_network_update_table(value, peer);
        }
        Content::RequestForTable { value } => {
            request_for_table(value, sender, peer);
        }
        Content::FindFile { song_name, instr } => {
            find_file(instr, song_name, peer, listener);
        }
        Content::ExistFile { song_name, id } => {
            exist_file(song_name, id, sender, peer);
        }
        Content::ExistFileResponse { song_name, id } => {
            exist_file_response(song_name, id, sender, peer);
        }
        Content::GetFile { key, instr } => {
            get_file(instr, key, sender, peer);
        }
        Content::GetFileResponse { value, instr, key } => {
            if get_file_response(&instr, &key, value, peer, sink).is_ok() {
                match instr {
                    FileInstructions::PLAY => listener.player_playing(Some(key)),
                    FileInstructions::GET => {
                        listener.local_database_changed(key, DOWNLOAD);
                    }
                    FileInstructions::ORDER => {
                        listener.local_database_changed(key, NEW);
                    }
                    _ => {}
                }
            }
        }
        Content::DeleteFileRequest { song_name } => {
            delete_file_request(&song_name, peer);
            listener.local_database_changed(song_name, DELETE);
        }
        Content::Response { .. } => {}
        Content::ExitPeer { addr } => {
            exit_peer(addr, peer);
        }
        Content::OrderSongRequest { song_name } => {
            order_song_request(song_name, peer);
        }
        Content::DeleteFromNetwork { name } => {
            delete_from_network(name, peer);
        }
        Content::SelfStatusRequest {} => {
            self_status_request(peer);
        }
        Content::StatusRequest {} => {
            status_request(sender, peer);
        }
        Content::StatusResponse { files, name } => {
            listener.notify_status(files, name);
        }
        Content::PlayAudioRequest { name, state } => {
            match state {
                MusicState::PLAY => {
                    if play_music(peer, &name, sink).is_ok() {
                        listener.player_playing(name);
                    }
                }
                MusicState::PAUSE => {
                    if pause_current_playing_music(sink).is_ok() {
                        println!("pause");
                    }
                }
                MusicState::STOP => {
                    if stop_current_playing_music(sink).is_ok() {
                        listener.player_stopped();
                    }
                }
                MusicState::CONTINUE => {
                    if continue_paused_music(sink).is_ok() {
                        println!("Continue");
                    }
                }
            };
        }
        Content::DroppedPeer { addr } => {
            dropped_peer(addr, peer);
        }
        Content::Heartbeat => {}
    }
}

pub fn send_write_request(
    target: SocketAddr,
    origin: SocketAddr,
    data: (String, Vec<u8>),
    redundant: bool,
    peer: &mut Peer,
) {
    let arc_peer = Arc::new(Mutex::new(peer.clone()));
    if let Err(e) = thread::Builder::new()
        .name("request_thread".to_string())
        .spawn(move || {
            let mut peer_lock = match arc_peer.lock() {
                Ok(p) => p,
                Err(e) => e.into_inner(),
            };
            let stream = match TcpStream::connect(target) {
                Ok(s) => s,
                Err(_e) => {
                    handle_lost_connection(target, &mut peer_lock);
                    return;
                }
            };
            if let true = redundant {
                let not = Notification {
                    content: Content::RedundantPushToDB {
                        key: data.0,
                        value: data.1,
                        from: origin.to_string(),
                    },
                    from: origin,
                };
                match serde_json::to_writer(&stream, &not) {
                    Ok(ser) => ser,
                    Err(e) => {
                        error!("Could not serialize {:?}, Error: {:?}", &not, e);
                        println!("Failed to serialize SendRequest {:?}", &not);
                    }
                };
            }
        })
    {
        error!("Request Thread could not be spawned: Error: {:?}", e);
    }
}

/// Selects a random `SocketAddr` from the `network_table` that is not equal to `own_ip`. Returns
/// `None` if there is no other `SocketAddr` in `network_table`.
fn other_random_target(
    network_table: &HashMap<String, SocketAddr>,
    own_ip: &SocketAddr,
) -> Option<SocketAddr> {
    if network_table.len() == 1 {
        return None;
    }
    let mut rng = rand::thread_rng();
    let mut index = rng.gen_range(0, network_table.len());
    let mut target = match network_table.values().nth(index) {
        Some(t) => t,
        None => {
            return None;
        }
    };
    while target == own_ip {
        index = rng.gen_range(0, network_table.len());
        target = match network_table.values().nth(index) {
            Some(t) => t,
            None => {
                return None;
            }
        };
    }
    Some(*target)
}

/// Communicate to the listener that we want to find the location of a given file
pub fn send_read_request(peer: &mut Peer, name: &str, instr: FileInstructions) {
    let not = Notification {
        content: Content::FindFile {
            instr,
            song_name: name.to_string(),
        },
        from: peer.ip_address,
    };
    if let Err(e) = peer.sender.send(not) {
        error!("Could not send notification {:?}", e);
    };
}

pub fn send_delete_peer_request(peer: &mut Peer) {
    let not = Notification {
        content: Content::ExitPeer {
            addr: peer.ip_address,
        },
        from: peer.ip_address,
    };
    if let Err(e) = peer.sender.send(not) {
        error!("Could not send notification {:?}", e);
    };
}

pub fn send_status_request(target: SocketAddr, from: SocketAddr, peer: &mut Peer) {
    let stream = match TcpStream::connect(target) {
        Ok(s) => s,
        Err(_e) => {
            handle_lost_connection(target, peer);
            return;
        }
    };

    let not = Notification {
        content: Content::StatusRequest {},
        from,
    };

    match serde_json::to_writer(&stream, &not) {
        Ok(ser) => ser,
        Err(_e) => {
            println!("Failed to serialize SendRequest {:?}", &not);
        }
    };
}

fn send_local_file_status(
    target: SocketAddr,
    files: Vec<String>,
    from: SocketAddr,
    peer_name: String,
) {
    let not = Notification {
        content: Content::StatusResponse {
            files,
            name: peer_name,
        },
        from,
    };

    tcp_request_with_notification(target, not);
}

pub fn send_play_request(name: Option<String>, peer: &mut Peer, state: MusicState) {
    let not = Notification {
        content: Content::PlayAudioRequest { name, state },
        from: peer.ip_address,
    };
    if let Err(e) = peer.sender.send(not) {
        error!("Could not send notification {:?}", e);
    };
}

fn handle_lost_connection(addr: SocketAddr, peer: &mut Peer) {
    peer.drop_peer_by_ip(&addr);
    let mut cloned_peer = peer.clone();
    for other_addr in peer.network_table.values() {
        if *other_addr != addr {
            send_dropped_peer_notification(*other_addr, addr, &mut cloned_peer)
        }
    }
}

/// Send a notification to the peer at `target` that the peer at `dropped_addr` has left the network
/// or was dropped.
/// # Parameters:
/// - `target`: `SocketAddr` of the Peer that should receive the notification
/// - `dropped_addr`: `SocketAddr` of the Peer that is not connected anymore
/// - `peer`: the local `Peer`
fn send_dropped_peer_notification(target: SocketAddr, dropped_addr: SocketAddr, peer: &mut Peer) {
    let stream = match TcpStream::connect(target) {
        Ok(s) => s,
        Err(_e) => {
            handle_lost_connection(target, peer);
            return;
        }
    };
    let not = Notification {
        content: Content::DroppedPeer { addr: dropped_addr },
        from: *peer.get_ip(),
    };
    if let Err(_e) = serde_json::to_writer(&stream, &not) {
        println!("Failed to serialize SendRequest {:?}", &not);
    }
}

/// Function to check file path to mp3 and saves to db afterwards
/// # Arguments:
///
/// * `name` - String including mp3 name (key in our database)
/// * `file_path` - Path to the mp3 file
/// * `peer` - Peer
///
/// # Returns:
/// Result
pub fn push_music_to_database(
    name: &str,
    file_path: &str,
    addr: SocketAddr,
    peer: &mut Peer,
) -> Result<(), io::Error> {
    // get mp3 file
    let path = Path::new(file_path);
    if path.exists() {
        let read_result = fs::read(path);
        match read_result {
            Ok(content) => {
                let not = Notification {
                    content: Content::PushToDB {
                        key: name.to_string(),
                        value: content,
                        from: addr.to_string(),
                    },
                    from: addr,
                };
                if let Err(e) = peer.sender.send(not) {
                    error!("Could not send notification {:?}", e);
                };
                return Ok(());
            }
            Err(err) => {
                println!("Error while parsing file");
                return Err(err);
            }
        }
    } else {
        println!("The file could not be found at this path: {:?}", path);
    }
    Err(io::Error::new(ErrorKind::NotFound, "File Path not found!"))
}

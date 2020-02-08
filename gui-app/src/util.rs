use meff::utils::{AppListener, ListenerInstr};
use meff::network;
use std::net::SocketAddr;
use meff::network::peer::Peer;
use meff::utils::Instructions::{REMOVE, GET};
use meff::network::{send_delete_peer_request, send_play_request, send_read_request, push_music_to_database};
use glib::{Sender};
use meff::audio::MusicState::{PAUSE, PLAY, STOP, CONTINUE};
use meff::audio::MusicState;
use std::collections::HashMap;
use std::sync::{Mutex, Arc};
use gtk::AccelGroupExt;
use std::borrow::BorrowMut;

//Music entertainment for friends application model
#[derive(Clone)]
pub struct MEFFM {
    pub peer: Option<Arc<Mutex<Peer>>>,
    pub sender: Option<Sender<(String, ListenerInstr)>>,
    pub is_playing: Arc<Mutex<bool>>,
}

impl AppListener for MEFFM {
    fn notify(&self) {
        println!("Hello world");
    }

    fn notify_status(&self, files: Vec<String>, name: String) {
        println!("Received status");
    }

    fn file_status_changed(&mut self, name: String, instr: ListenerInstr) {
        println!("new_file_saved");
        //@TODO remove unwrap
        self.sender.as_ref().unwrap().send((name, instr));
    }

    fn player_playing(&mut self, title: Option<String>) {
        *self.is_playing.lock().unwrap() = true;
    }

}

impl MEFFM {
    pub fn new() -> MEFFM {
        MEFFM {peer: None, sender: None, is_playing: Arc::new(Mutex::new(false))}
    }

    pub fn set_sender(&mut self, sender: Sender<(String, ListenerInstr)>) {
        self.sender = Some(sender);
    }

    //@TODO return result
    pub fn start(&mut self, name: String, port: String, ip: Option<SocketAddr>) {
        let peer = match network::startup(&name, &port, ip, Box::new(self.clone())) {
            Ok(p) => p,
            Err(_e) => {
                //@TODO exit programm
                return;
            } // error!("Could not join network {:?}", e);
        };
        self.peer = Some(peer);
    }

    pub fn push(&mut self, path: String, title: String) {
        let peer_unlock = self.peer.as_ref().unwrap().lock().unwrap();
        let mut peer_clone = peer_unlock.clone();

        let ip = peer_clone.ip_address;
        match push_music_to_database(&title, &path, ip,  &mut peer_clone) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Failed to push {} to database", path);
            }
        };
        drop(peer_unlock);
    }

    pub fn remove_title(&mut self, title: String) {
        let peer_unlock = self.peer.as_ref().unwrap().lock().unwrap();
        let mut peer_clone = peer_unlock.clone();

        send_read_request(&mut peer_clone, &title, REMOVE);
        drop(peer_unlock);
    }

    fn music_control(&mut self, song: Option<String>, instr: MusicState) {
        let peer_unlock = self.peer.as_ref().unwrap().lock().unwrap();
        let mut peer_clone = peer_unlock.clone();
        send_play_request(song, &mut peer_clone, instr);
        drop(peer_unlock);
    }

    pub fn status(&mut self) -> HashMap<String, SocketAddr> {
        let peer_unlock = self.peer.as_ref().unwrap().lock().unwrap();
        let mut peer_clone = peer_unlock.clone();

        let list = peer_clone.network_table;
        drop(peer_unlock);
        list
    }

    pub fn stream(&mut self, search: String) {
        self.music_control(Some(search), PLAY);
    }

    pub fn download(&mut self, title: String) {
        let peer_unlock = self.peer.as_ref().unwrap().lock().unwrap();
        let mut peer_clone = peer_unlock.clone();
        send_read_request(&mut peer_clone, &title, GET);
        drop(peer_unlock);
    }

    pub fn play(&mut self, title: Option<String>) {
        if *self.is_playing.lock().unwrap() {
            self.music_control(None,CONTINUE);
        } else {
            self.music_control(title, PLAY);
        }
    }

    pub fn pause(&mut self) {
        self.music_control(None,PAUSE);
    }

    pub fn stop(&mut self) {
        *self.is_playing.lock().unwrap() = false;
        self.music_control(None,STOP);
    }

    pub fn quit(&mut self) {
        let peer_unlock = self.peer.as_ref().unwrap().lock().unwrap();
        let mut peer_clone = peer_unlock.clone();

        send_delete_peer_request(&mut peer_clone);
        drop(peer_unlock);
    }
}


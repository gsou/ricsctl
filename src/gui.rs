
use gtk::prelude::*;
use gio::prelude::*;

use super::rlua::Lua;
use super::rlua;
use super::gtk;
use super::gio;
use super::glib;
use super::server;
use super::rics;

use std::rc::{Rc};
use std::path::PathBuf;
use std::cell::{RefCell, RefMut};
use std::sync::{Arc, RwLock, Mutex};
use std::thread;

/// Filter a can message. Return wheter to keep the message with the bool, and the Parsed Data as a String. The last flag is the color with which to highlight the message
fn filter_can(lua: &Lua, id: u32, data: Vec<u8>) -> (bool, String, String) {
    lua.context(|ctx| {
        match ctx.globals().get("filter_can").and_then(|f: rlua::Function| f.call((id, data))) {
            Ok(ret) => ret,
            Err(err) => { error!("{}", err); (true, "".to_string(), "".to_string())}
        }
    })
}

/// Apply the filter in filter_can to the entire can_store
fn apply_filter(lua: &Lua, can_store: &mut gtk::ListStore) {
    let mut to_remove = vec![];
    {
        let to_remove = &mut to_remove;
        can_store.foreach(move |m, p, i| {
            // TODO check for the from_str_radix unwrap
            let id = u32::from_str_radix(&m.get_value(i, 0).downcast::<String>().unwrap().get().unwrap(), 16).unwrap();
            let len = u8::from_str_radix(&m.get_value(i, 1).downcast::<String>().unwrap().get().unwrap(), 16).unwrap();
            let mut dat = vec![];
            for j in 0..len {
                dat.push(u8::from_str_radix(&m.get_value(i, (2 + j).into()).downcast::<String>().unwrap().get().unwrap(), 16).unwrap());
            }
            let (fil,data,color) = filter_can(lua, id, dat);
            if fil {
                m.downcast_ref::<gtk::ListStore>().unwrap().set_value(i, 10, &data.to_value());
                m.downcast_ref::<gtk::ListStore>().unwrap().set_value(i, 11, &color.to_value());
            } else {
                let iter = m.get_iter(p).unwrap();
                to_remove.push(iter)
            }
            false
        });
    }
    for i in to_remove.iter().rev() {
        can_store.downcast_ref::<gtk::ListStore>().unwrap().remove(&i);
    }

}


fn load_script(lua: &Lua, filename: PathBuf) {
    lua.context(|ctx| {
        match std::fs::read_to_string(filename) {
            Ok(s) =>
                match ctx.load(&s).exec() {
                    Ok(_) => { info!("Script loaded"); () },
                    Err(err) => error!("Error loading script: {}", err),
                },
            Err(err) => error!("Error opening script: {}", err),
        }
    });
}

fn dialog_open_file(window: &gtk::Window, title: &str, ok: &str, act: gtk::FileChooserAction) -> Option<PathBuf>{
    let dialog = gtk::FileChooserDialog::new(Some(title), Some(window), act);
    dialog.add_buttons(&[
        (ok, gtk::ResponseType::Ok),
        ("Cancel", gtk::ResponseType::Cancel),
    ]);

    let ret = if dialog.run() == gtk::ResponseType::Ok {
        if let Some(filename) = dialog.get_filename() {
            Some(filename)
        } else {
            None
        }
    } else {
        None
    };
    dialog.close();
    ret
}




pub fn gui_main() {
    if let Err(err) = gtk::init() {
        error!("{}", err);
        return;
    }

    let lua = rlua::Lua::new();
    let lua = Rc::new(lua);

    let glade_src = include_str!("../gui.glade");
    let builder = gtk::Builder::from_string(glade_src);

    // Variables
    // XXX Rc sufficient here ?
    let server : Rc<RefCell<Option<server::RICSServer>>> = Rc::new(RefCell::new(None));
    let window = Rc::new(RefCell::new(builder.get_object::<gtk::Window>("RICSWIN").unwrap()));
    let can_store = Rc::new(RefCell::new(builder.get_object::<gtk::ListStore>("can_store").unwrap()));
    let tree_view = Rc::new(RefCell::new(builder.get_object::<gtk::TreeView>("tree_view").unwrap()));
    let filter_cont = Rc::new(builder.get_object::<gtk::CheckMenuItem>("filter_cont").unwrap());
    let status = Rc::new(builder.get_object::<gtk::Statusbar>("status").unwrap());

    // Server connect
    let server_clone = Rc::clone(&server);
    let status_clone = Rc::clone(&status);
    builder.get_object::<gtk::MenuItem>("svr_conn_def").unwrap().connect_activate(move |_| {
        let mut svr: RefMut<_> = server_clone.borrow_mut();
        *svr = server::RICSServer::new().ok();
        if svr.is_none() {
            warn!("Could not connect to default server");
            status_clone.push(0, "Error ! Could not connect to server");
        }
        svr.as_mut().map(|s| {status_clone.push(0, "Connected to server"); s.connect(true); });
    });

    // File

    let can_store_clone = Rc::clone(&can_store);
    builder.get_object::<gtk::MenuItem>("file_new").unwrap().connect_activate(move |_| {can_store_clone.borrow().clear();});

    let window_clone = Rc::clone(&window);
    let can_store_clone = Rc::clone(&can_store);
    let tree_view_clone = Rc::clone(&tree_view);
    builder.get_object::<gtk::MenuItem>("file_open").unwrap().connect_activate(move |_| {(|| -> Option<()> {
        if let Some(file) = dialog_open_file(&window_clone.borrow_mut(), "Open File", "Open", gtk::FileChooserAction::Open) {
            let mut rdr = csv::Reader::from_path(file).unwrap();
            let can_store = can_store_clone.borrow_mut();

            can_store.clear();


            debug!("Opening file");
            for result in rdr.records() {
                let record = result.ok()?;
                trace!("Record {:?}", record);

                can_store.insert_with_values(None, &(0..13 as u32).collect::<Vec<_>>()[..],
                                             &[&record.get(1).unwrap().to_string(),
                                               &record.get(2).unwrap().to_string(),
                                               &record.get(3).unwrap_or("").to_string(),
                                               &record.get(4).unwrap_or("").to_string(),
                                               &record.get(5).unwrap_or("").to_string(),
                                               &record.get(6).unwrap_or("").to_string(),
                                               &record.get(7).unwrap_or("").to_string(),
                                               &record.get(8).unwrap_or("").to_string(),
                                               &record.get(9).unwrap_or("").to_string(),
                                               &record.get(10).unwrap_or("").to_string(),
                                               // Lua parsing of messages
                                               &record.get(11).unwrap_or("").to_string(),
                                               &record.get(12).unwrap_or("").to_string(),
                                               &record.get(0).unwrap_or("").to_string(),
                                             ]);

                let tree_view = tree_view_clone.borrow_mut();
                tree_view.set_model(Some(&*can_store));
                trace!("Append Message to interface");

            }
        }
        debug!("Done opening file");
        Some(())
    })();
    });

    let window_clone = Rc::clone(&window);
    let lua_clone = Rc::clone(&lua);
    builder.get_object::<gtk::MenuItem>("filter_load").unwrap().connect_activate(move |_| {
        if let Some(file) = dialog_open_file(&window_clone.borrow_mut(), "Load Script", "Load", gtk::FileChooserAction::Open) {
            load_script(&*lua_clone, file);
        }
    });

    let window_clone = Rc::clone(&window);
    let lua_clone = Rc::clone(&lua);
    let can_store_clone = Rc::clone(&can_store);
    let status_clone = Rc::clone(&status);
    builder.get_object::<gtk::MenuItem>("file_save").unwrap().connect_activate(move |_| {
        if let Some(file) = dialog_open_file(&window_clone.borrow_mut(), "Save", "Save", gtk::FileChooserAction::Save) {
            let can_store = can_store_clone.borrow_mut();
            debug!("Opening file");
            if let Ok(mut wrt) = csv::Writer::from_path(file) {
            can_store.foreach(move |m, p, i| {
                let vec: Vec<String> = [12,0,1,2,3,4,5,6,7,8,9,10,11].iter().map(|n| m.get_value(i, *n).downcast::<String>().unwrap().get().unwrap()).collect();
                wrt.write_record(&vec[..]).unwrap();
                wrt.flush();
                false
            });
            debug!("Saving done");
            status_clone.push(0, "Data saved");
            } else {
                error!("Error saving file");
                status_clone.push(0, "Error ! Data NOT saved");

            }

        }
    });

    let can_store_clone = Rc::clone(&can_store);
    let lua_clone = Rc::clone(&lua);
    let status_clone = Rc::clone(&status);
    builder.get_object::<gtk::MenuItem>("filter_apply").unwrap().connect_activate(move |_| {
        apply_filter(&*lua_clone, &mut can_store_clone.borrow_mut());
        status_clone.push(0, "Filtered !");
    });

    // Listening thread.
    let conn_fork = builder.get_object::<gtk::MenuItem>("conn_fork").unwrap();
    let server_clone = Rc::clone(&server);
    let filter_cont_clone = Rc::clone(&filter_cont);
    let lua_clone = Rc::clone(&lua);
    let status_clone = Rc::clone(&status);
    let tree_view_clone = Rc::clone(&tree_view);
    conn_fork.connect_activate(move |_| {
        let mut svr: RefMut<_> = server_clone.borrow_mut();
        if let Some(resp) = svr.as_mut().map(|s|s.listen_response()) {
            let can_store = Rc::clone(&can_store);
            let tree_view_clone = Rc::clone(&tree_view_clone);
            let lua_clone = Rc::clone(&lua_clone);
            let filter_cont_clone = Rc::clone(&filter_cont);
            status_clone.push(0, "Starting listening thread");
            gtk::idle_add(move || {
                if let Ok(packet) = resp.try_recv() {
                    if packet.has_data() {
                        let data = packet.get_data();
                        if data.get_field_type() == rics::RICS_Data_RICS_DataType::CAN {
                            // dataa.get_id();
                            // let n = data.get_data().len();
                            // data.get_data();
                            let can_store = can_store.borrow_mut();

                            let columns = data.get_data().len();
                            let len = data.get_data().len() as u8;

                            let (fil,parsed,color) = if filter_cont_clone.get_active() {
                                filter_can(&*lua_clone, data.get_id() as u32, data.get_data().to_vec())
                            } else {
                                (true, "".to_string(), "black".to_string())
                            };

                            if fil {
                                let time = std::time::SystemTime::now();
                                let datetime: chrono::DateTime<chrono::offset::Local> = time.into();
                                can_store.insert_with_values(None, &(0..13 as u32).collect::<Vec<_>>()[..],
                                                             &[&format!("{:x}", data.get_id()), &format!("{:x}",len),
                                                               &data.get_data().get(0).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(1).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(2).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(3).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(4).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(5).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(6).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               &data.get_data().get(7).map(|x|format!("{:x}",x)).unwrap_or("".to_string()),
                                                               // Lua parsing of messages
                                                               &parsed,
                                                               &color,
                                                               &format!("{}", datetime.format("%Y-%m-%d %T%.3f")),
                                                             ]);
                            }
                            let tree_view = tree_view_clone.borrow_mut();
                            tree_view.set_model(Some(&*can_store));
                            trace!("Append Message to interface");

                        }
                    }
                }
                Continue(true)
            });
        }
    });


    // Start application
    window.borrow_mut().show_all();
    gtk::main();
}

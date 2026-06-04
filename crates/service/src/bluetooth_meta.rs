use std::collections::HashMap;
use std::time::Duration;

use carplay_protocol::TrackInfo;
use dbus::arg::{self, PropMap};
use dbus::blocking::Connection;
use tokio::sync::mpsc::UnboundedSender;

pub fn watch_loop(tx: UnboundedSender<Option<TrackInfo>>) {
    let conn = match Connection::new_system() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[bt-meta] D-Bus connection failed: {}", e);
            return;
        }
    };

    let mut last: Option<TrackInfo> = None;

    loop {
        let current = get_track(&conn);
        if current != last {
            last = current.clone();
            let _ = tx.send(current);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

// a{oa{sa{sv}}} — the return type of GetManagedObjects
type ManagedObjects = HashMap<dbus::Path<'static>, HashMap<String, PropMap>>;

fn get_track(conn: &Connection) -> Option<TrackInfo> {
    let proxy = conn.with_proxy("org.bluez", "/", Duration::from_secs(2));

    let (objects,): (ManagedObjects,) = proxy
        .method_call("org.freedesktop.DBus.ObjectManager", "GetManagedObjects", ())
        .ok()?;

    for (_path, ifaces) in &objects {
        let player = ifaces.get("org.bluez.MediaPlayer1")?;

        let track_var = player.get("Track")?;
        // The Track property is a{sv} — cast the inner RefArg to PropMap
        let track: &PropMap = arg::cast(&*track_var.0)?;

        let title = arg::prop_cast::<String>(track, "Title").cloned();
        let artist = arg::prop_cast::<String>(track, "Artist").cloned();
        let album = arg::prop_cast::<String>(track, "Album").cloned();
        let duration_ms = arg::prop_cast::<u32>(track, "Duration").map(|&ms| ms as u64);

        if title.is_none() && artist.is_none() && album.is_none() {
            return None;
        }

        return Some(TrackInfo { title, artist, album, duration_ms });
    }

    None
}

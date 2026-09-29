//! El relé de la sincronización (ADR 0007): reenvía marcos entre las máquinas del mismo grupo.
//! No puede leerlos (van cifrados de punta a punta); solo ve el id de grupo y los tamaños.
//!
//! Protocolo: cada marco es 4 bytes de largo (big endian) + contenido. El primero de cada
//! conexión es `JSR1` + el id de grupo (16 bytes); los siguientes se reenvían, tal cual, a las
//! otras conexiones del grupo.
//!
//! Sin `--publico` escucha solo en 127.0.0.1, que es lo que ven las máquinas virtuales de QEMU
//! como 10.0.2.2.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MAX_FRAME: usize = 9 * 1024 * 1024;
const MAX_PER_GROUP: usize = 8;
const MAX_CONNECTIONS: usize = 256;

/// Por grupo: cada conexión con su cola de salida y su socket (para cortarla si no lee).
type Groups = Arc<Mutex<HashMap<[u8; 16], Vec<(u64, SyncSender<Vec<u8>>, TcpStream)>>>>;

fn read_frame(s: &mut TcpStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; 4 + len];
    buf[..4].copy_from_slice(&(len as u32).to_be_bytes());
    s.read_exact(&mut buf[4..]).ok()?;
    Some(buf)
}

fn client(mut s: TcpStream, id: u64, groups: Groups) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let Some(hello) = read_frame(&mut s) else {
        return;
    };
    if hello.len() != 4 + 20 || &hello[4..8] != b"JSR1" {
        return;
    }
    let group: [u8; 16] = hello[8..24].try_into().expect("16 bytes");
    let _ = s.set_read_timeout(None);
    // Cada conexión escribe desde su propio hilo; la cola es acotada: si una máquina no lee,
    // se la desconecta en vez de llenar la memoria del relé.
    let (tx, rx) = sync_channel::<Vec<u8>>(64);
    let (Ok(mut writer), Ok(handle)) = (s.try_clone(), s.try_clone()) else {
        return;
    };
    {
        let mut g = groups.lock().expect("lock");
        let members = g.entry(group).or_default();
        if members.len() >= MAX_PER_GROUP {
            return;
        }
        members.push((id, tx.clone(), handle));
        println!(
            "relé: conexión {id} en el grupo {} ({} en total)",
            short(&group),
            members.len()
        );
    }
    thread::spawn(move || {
        for f in rx {
            if writer.write_all(&f).is_err() {
                break;
            }
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });
    drop(tx);
    while let Some(f) = read_frame(&mut s) {
        let g = groups.lock().expect("lock");
        for (other, tx, sock) in g.get(&group).into_iter().flatten() {
            if *other != id && matches!(tx.try_send(f.clone()), Err(TrySendError::Full(_))) {
                // No lee: se la corta; al reconectar se pone al día con el manifiesto.
                let _ = sock.shutdown(std::net::Shutdown::Both);
            }
        }
    }
    let mut g = groups.lock().expect("lock");
    if let Some(m) = g.get_mut(&group) {
        m.retain(|(o, _, _)| *o != id);
        if m.is_empty() {
            g.remove(&group);
        }
    }
    println!("relé: se fue la conexión {id}");
}

fn short(g: &[u8; 16]) -> String {
    g[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Atiende conexiones para siempre (cada una en su hilo).
pub fn serve(listener: TcpListener) {
    let groups: Groups = Arc::default();
    let live = Arc::new(Mutex::new(0usize));
    for (id, s) in listener.incoming().flatten().enumerate() {
        {
            let mut n = live.lock().expect("lock");
            if *n >= MAX_CONNECTIONS {
                continue;
            }
            *n += 1;
        }
        let (groups, live) = (groups.clone(), live.clone());
        thread::spawn(move || {
            client(s, id as u64, groups);
            *live.lock().expect("lock") -= 1;
        });
    }
}

/// Escucha en `host:port`.
pub fn bind(public: bool, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((if public { "0.0.0.0" } else { "127.0.0.1" }, port))
}

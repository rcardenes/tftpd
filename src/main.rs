use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

use clap::{arg, command, value_parser};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt},
    net::UdpSocket
};
use anyhow::Result;

use tftpd::{parse_message, ErrorCode, Message, Mode};

const DEFAULT_PORT: &str = "69";
const DEFAULT_STATIC_ROOT: &str = "/srv/tftp/static";
const BLOCK_SIZE: u64 = 512;

#[derive(Debug)]
struct Config {
    port: u16,
    static_root: PathBuf,
}

fn get_config() -> Result<Config> {
    let matches = command!()
        .arg(arg!(-p --port <PORT> "Listening port")
                .value_parser(value_parser!(u16))
                .default_value(DEFAULT_PORT))
        .arg(arg!(-r --root <ROOT> "Root directory containing files to be served")
                .value_parser(value_parser!(PathBuf))
                .default_value(DEFAULT_STATIC_ROOT))
        .get_matches();

    let port = *matches.get_one::<u16>("port").unwrap();
    let static_root = matches.get_one::<PathBuf>("root").unwrap().to_owned();

    Ok(Config {
        port,
        static_root,
    })
}

struct TransferInfo {
    file: File,
    last_block: u16,
}

struct Tracker {
    active: HashMap<SocketAddr, TransferInfo>,
}

impl Tracker {
    fn new() -> Self {
        Tracker {
            active: HashMap::new()
        }
    }

    fn add(&mut self, addr: SocketAddr, file: File) {
        self.active.insert(addr, TransferInfo { file, last_block: 1 });
    }

    fn ack(&mut self, addr: &SocketAddr, block: u16) -> bool {
        if let Some(info) = self.active.get_mut(addr) {
            if info.last_block == block {
                info.last_block = info.last_block + 1;
                return true;
            }
        }

        false
    }

    fn del(&mut self, addr: &SocketAddr) {
        if let Some(value) = self.active.remove(addr) {
            // This should be automatic, but let's be explicit about it
            drop(value.file);
        }
    }

    async fn read_block(&mut self, addr: &SocketAddr) -> Option<Message> {
        if let Some(info) = self.active.get_mut(addr) {
            let mut buffer = [0 as u8; BLOCK_SIZE as usize];
            let offset = ((info.last_block - 1) as u64) * BLOCK_SIZE;
            match info.file.seek(std::io::SeekFrom::Start(offset)).await {
                Err(error) => {
                    // TODO: This would have consequences...
                    eprintln!("Error when seeking: {error:?}");
                    return None;
                }
                _ => {}
            };
            let len = match info.file.read(&mut buffer).await {
                Ok(len) => len,
                Err(error) => {
                    // TODO: This would have consequences...
                    eprintln!("Error when reading: {error:?}");
                    return None;
                }
            };

            if len > 0 {
                Some(Message::Data {
                    block: info.last_block,
                    payload: buffer[..len].into(),
                })
            } else {
                None
            }
        } else {
            None
        }
    }
}

async fn open_file(config: &Config, filename: &str) -> Result<File, Message> {
    let mut path = config.static_root.clone();
    path.push(filename);
    // Verify that appending the filename hasn't directed out of the
    // filesystem root (can happen when the path is normalized, for
    // example suplying relative paths)
    if !path.starts_with(&config.static_root) {
        return Err(ErrorCode::AccessViolation.into_explicit_message("Illegal path"));
    }

    Ok(match OpenOptions::new().read(true).open(path).await {
        Ok(file) => file,
        Err(error) => {
            return Err(match error.kind() {
                std::io::ErrorKind::NotFound => {
                    ErrorCode::FileNotFound.into_message()
                }
                std::io::ErrorKind::PermissionDenied => {
                    ErrorCode::AccessViolation.into_explicit_message("Permission denied")
                }
                _ => ErrorCode::NotDefined.into_explicit_message(&format!("{error}")),
            })
        }
    })
}

async fn send_block(tracker: &mut Tracker, sock: &UdpSocket, addr: SocketAddr) {
    let packet = tracker.read_block(&addr).await
        .and_then(|msg| Some(msg.into_packet()));
// // TODO: None should be the case when there's nothing else to send...
// //            Some(ErrorCode::NotDefined
// //                 .into_message()
// //                 .into_packet())
//         .or_else(|| {
//             tracker.del(&addr);
//         }).unwrap();
    if let Some(p) = packet {
        sock.send_to(&p, addr.clone()).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = get_config()?;
    let mut tracker = Tracker::new();

    let sock = UdpSocket::bind(("127.0.0.1", config.port)).await?;

    let mut buf = [0; 1024];
    loop {
        let (_, addr) = sock.recv_from(&mut buf).await?;

        match parse_message(&buf) {
            Ok(message) => {
                match message {
                    Message::Write { .. } => {
                        sock.send_to(
                            ErrorCode::IllegalOperation
                                .into_explicit_message("No write permission")
                                .into_packet().as_ref(),
                            addr).await?;
                    }
                    Message::Read { filename, mode } => {
                        if mode != Mode::Octet {
                            eprintln!("Not a supported mode...");
                            sock.send_to(
                                &ErrorCode::IllegalOperation
                                    .into_explicit_message("Only Octet transfers are supported")
                                    .into_packet(),
                                addr).await;
                        } else {
                            match open_file(&config, &filename).await {
                                Ok(file) => {
                                    tracker.add(addr.clone(), file);
                                    send_block(&mut tracker, &sock, addr).await;
                                }
                                Err(errmsg) => {
                                    sock.send_to(&errmsg.into_packet(), addr).await;
                                }
                            }
                        }
                    }
                    Message::Ack(block) => {
                        tracker.ack(&addr, block);
                        send_block(&mut tracker, &sock, addr).await;
                    }
                    _ => {}
                }
            },
            Err(error) => {},
        }
    }
}

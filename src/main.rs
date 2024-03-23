use std::path::PathBuf;

use clap::{arg, command, value_parser};
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncReadExt,
    net::UdpSocket, time::timeout
};
use anyhow::Result;

use tftpd::{parse_message, ErrorCode, Message, Mode};

const DEFAULT_PORT: &str = "69";
const DEFAULT_STATIC_ROOT: &str = "/srv/tftp/static";
const BLOCK_SIZE: usize = 512;
const MAX_ATTEMPTS: usize = 5;
const DEFAULT_TIMEOUT: u64 = 3000; // milliseconds

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

async fn read_block(file: &mut File, block_size: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0; block_size];
    let len = file.read(&mut buffer).await?;

    Ok(buffer[..len].to_vec())
}

async fn worker_task(sock: UdpSocket, mut file: File) {
    let block_size = BLOCK_SIZE;
    let tout = tokio::time::Duration::from_millis(DEFAULT_TIMEOUT);

    let mut current_block: u16 = 0;
    let mut read_buffer = vec![0; block_size];
    loop {
        current_block += 1;
        let payload = match read_block(&mut file, block_size).await {
            Ok(data) => data,
            Err(_) => {
                sock.send(&ErrorCode::NotDefined.into_message().into_packet()).await;
                break;
            }
        };
        let payload_len = payload.len();

        let message = Message::Data { block: current_block, payload }.into_packet();
        let mut failed_attempts = 0;
        let mut waiting_for_ack = false;
        while failed_attempts < MAX_ATTEMPTS {
            if !waiting_for_ack {
                sock.send(&message).await;
                waiting_for_ack = true;
            } else if timeout(tout, sock.recv(&mut read_buffer)).await.is_ok() {
                if let Ok(message) = parse_message(&read_buffer) {
                    match message {
                        Message::Ack(block_id) => {
                            if block_id == current_block {
                                break;
                            }
                        }
                        _ => {
                            sock.send(&ErrorCode::IllegalOperation
                                      .into_message()
                                      .into_packet()).await;
                        }
                    };
                }
            } else {
                failed_attempts += 1;
                eprintln!("Timeout (failed: {failed_attempts}/{MAX_ATTEMPTS})");
                waiting_for_ack = false;
            }
        }

        if failed_attempts >= MAX_ATTEMPTS {
            return;
        }

        if payload_len < block_size {
            break;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = get_config()?;

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
                            sock.send_to(
                                &ErrorCode::IllegalOperation
                                    .into_explicit_message("Only Octet transfers are supported")
                                    .into_packet(),
                                addr).await;
                        } else {
                            match open_file(&config, &filename).await {
                                Ok(file) => {
                                    // TODO: We should look for errors here...
                                    let sock = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
                                    sock.connect(addr).await.unwrap();

                                    tokio::spawn(worker_task(sock, file));
                                }
                                Err(errmsg) => {
                                    sock.send_to(&errmsg.into_packet(), addr).await;
                                }
                            }
                        }
                    }
                    Message::Ack(block) => {
                    }
                    _ => {}
                }
            },
            Err(error) => {},
        }
    }
}

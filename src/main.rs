use std::{net::SocketAddr, path::PathBuf};

use clap::{arg, command, value_parser};
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncReadExt,
    net::UdpSocket,
    time::{Duration, timeout}
};
use anyhow::{bail, Result};

use tftpd::{parse_message, ErrorCode, Message, Mode, TftpOption};

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

enum Dest {
    Fixed,
    Addr(SocketAddr),
}

async fn send_error(sock: &UdpSocket, msg: Message, to: Dest) {
    let packet = msg.into_packet();
    let res = match to {
        Dest::Fixed => sock.send(&packet).await,
        Dest::Addr(addr) => sock.send_to(&packet, addr).await,
    };

    match res {
        Err(error) => eprintln!("While trying to send an error message: {error:?}"),
        _ => {}
    }
}

fn get_block_size(options: &[TftpOption]) -> usize {
    for opt in options {
        match opt {
            TftpOption::BlockSize(bls) => { return *bls as usize },
            _ => {}
        }
    }

    BLOCK_SIZE
}

fn get_timeout(options: &[TftpOption]) -> u64 {
    for opt in options {
        match opt {
            TftpOption::Timeout(tout) => { return *tout as u64 },
            _ => {}
        }
    }

    DEFAULT_TIMEOUT
}

fn get_transfer_size(options: &[TftpOption]) -> Option<u64> {
    for opt in options {
        match opt {
            TftpOption::TransferSize(tsize) => { return Some(*tsize) },
            _ => {}
        }
    }

    None
}

async fn packet_and_ack(sock: &UdpSocket, block: u16, packet: &Vec<u8>, block_size: usize, tout: Duration) -> Result<()> {
    let mut read_buffer = vec![0; block_size];
    let mut failed_attempts = 0;
    let mut waiting_for_ack = false;
    while failed_attempts < MAX_ATTEMPTS {
        if !waiting_for_ack {
            if !sock.send(&packet).await.is_ok() {
                // Abort, something really wrong happened here
                bail!("Critical error attemting to send packet");
            }
            waiting_for_ack = true;
        } else if timeout(tout, sock.recv(&mut read_buffer)).await.is_ok() {
            if let Ok(message) = parse_message(&read_buffer) {
                match message {
                    Message::Ack(block_id) => {
                        if block_id == block {
                            break;
                        }
                    }
                    _ => {
                        send_error(
                            &sock,
                            ErrorCode::IllegalOperation.into_message(),
                            Dest::Fixed
                            ).await;
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
        bail!("Too many retries")
    }

    Ok(())
}

async fn worker_task(sock: UdpSocket, mut file: File, options: Vec<TftpOption>) {
    let block_size = get_block_size(&options);
    let tout = Duration::from_millis(get_timeout(&options));

    if options.len() > 0 {
        if let Some(tsize) = get_transfer_size(&options) {
            let fsize = file.metadata().await.unwrap().len();

            if tsize > fsize {
                send_error(
                    &sock,
                    ErrorCode::OptionNegotiationError
                        .into_explicit_message("File too large"),
                    Dest::Fixed).await;
                return;
            }
        }

        let msg = Message::OptionAck { options }.into_packet();
        match packet_and_ack(&sock, 0, &msg, block_size, tout).await {
            Err(error) => eprintln!("{error}"),
            _ => {}
        }
    }

    let mut current_block: u16 = 0;
    loop {
        current_block += 1;
        let payload = match read_block(&mut file, block_size).await {
            Ok(data) => data,
            Err(_) => {
                send_error(&sock, ErrorCode::NotDefined.into_message(), Dest::Fixed).await;
                break;
            }
        };
        let payload_len = payload.len();

        let message = Message::Data { block: current_block, payload }.into_packet();

        match packet_and_ack(&sock, current_block, &message, block_size, tout).await {
            Err(error) => eprintln!("{error}"),
            _ => {}
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
                    Message::Read { filename, mode, options } => {
                        if mode != Mode::Octet {
                            send_error(
                                &sock, 
                                ErrorCode::IllegalOperation
                                    .into_explicit_message("Only Octet transfers are supported"),
                                Dest::Addr(addr),
                                ).await
                        } else {
                            match open_file(&config, &filename).await {
                                Ok(file) => {
                                    // TODO: We should look for errors here...
                                    let sock = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
                                    sock.connect(addr).await.unwrap();

                                    tokio::spawn(worker_task(sock, file, options));
                                }
                                Err(errmsg) => {
                                    send_error(&sock, errmsg, Dest::Addr(addr)).await;
                                }
                            }
                        }
                    }
                    msg => {
                        if !sock.send_to(
                            &ErrorCode::IllegalOperation
                                .into_message()
                                .into_packet(),
                            addr).await.is_ok()
                        {
                            eprintln!("Error trying to answer to illegal message: {msg:?}");
                        }
                    }
                }
            },
            Err(error) => {
                eprintln!("While parsing message: {error}");
            },
        }
    }
}

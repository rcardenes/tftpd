#[derive(Debug)]
enum PacketType {
    ReadRequest,
    WriteRequest,
    Data,
    Acknowledgement,
    Error,
}

impl TryFrom<u16> for PacketType {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, <PacketType as TryFrom<u16>>::Error> {
        match value {
            1 => Ok(PacketType::ReadRequest),
            2 => Ok(PacketType::WriteRequest),
            3 => Ok(PacketType::Data),
            4 => Ok(PacketType::Acknowledgement),
            5 => Ok(PacketType::Error),
            _ => Err(())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    NetAscii,
    Octet,
    Mail,
}

impl TryFrom<&str> for Mode {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match (value.to_lowercase()).as_str() {
            "netascii" => Ok(Mode::NetAscii),
            "octet" => Ok(Mode::Octet),
            "mail" => Ok(Mode::Mail),
            _ => Err(())
        }
    }
}

#[derive(Debug)]
pub enum ErrorCode {
    NotDefined,
    FileNotFound,
    AccessViolation,
    DiskFull,
    IllegalOperation,
    UnknownTransferId,
    FileAlreadyExists,
    NoSuchUser,
    OptionNegotiationError,
}

impl ErrorCode {
    pub fn into_message(self) -> Message {
        Message::Error {
            message: match &self {
                ErrorCode::NotDefined => "Not defined",
                ErrorCode::FileNotFound => "File not found",
                ErrorCode::AccessViolation => "Access violation",
                ErrorCode::DiskFull => "Disk full",
                ErrorCode::IllegalOperation => "Illegal operation",
                ErrorCode::UnknownTransferId => "Unknown TID",
                ErrorCode::FileAlreadyExists => "File already exists",
                ErrorCode::NoSuchUser => "No such user",
                ErrorCode::OptionNegotiationError => "Error during option negotiation",
            }.into(),
            code: self,
        }
    }

    pub fn into_explicit_message(self, message: &str) -> Message {
        Message::Error { code: self, message: message.into() }
    }
}

#[derive(Debug)]
pub enum Message {
    Read { filename: String, mode: Mode, options: Vec<TftpOption> },
    Write { filename: String, mode: Mode, options: Vec<TftpOption> },
    Data { block: u16, payload: Vec<u8> },
    Ack(u16),
    Error { code: ErrorCode, message: String },
    OptionAck { options: Vec<TftpOption> },
}

impl Message {
    fn read_from_arguments(args: Arguments) -> Self {
        Message::Read {
            filename: args.filename,
            mode: args.mode,
            options: args.options,
        }
    }

    fn write_from_arguments(args: Arguments) -> Self {
        Message::Write {
            filename: args.filename,
            mode: args.mode,
            options: args.options,
        }
    }

    pub fn into_packet(self) -> Vec<u8> {
        match self {
            // Message::Read { filename, mode } => todo!(),
            // Message::Write { filename, mode } => todo!(),
            Message::Data { block, payload } => {
                3_u16.to_be_bytes().into_iter()
                    .chain(block.to_be_bytes())
                    .chain(payload)
                    .collect()
            }
            // Message::AckMessage(_) => todo!(),
            Message::Error { code, message } => {
                5_u16.to_be_bytes().into_iter()
                    .chain((code as u16).to_be_bytes())
                    .chain(message.bytes())
                    .chain([0])
                    .collect()
            }
            Message::OptionAck { options } => {
                let encoded_options = options.iter().map(|op| op.encode());
                6_u16.to_be_bytes().into_iter()
                    .chain(encoded_options.flatten())
                    .collect()
            }
            _ => todo!()
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    CorruptPacket(String),
    InvalidOpcode(u16),
    InvalidString(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::CorruptPacket(string) => write!(f, "Corrupt packet: {string}"),
            ParseError::InvalidOpcode(opcode) => write!(f, "Invalid opcode: {opcode}"),
            ParseError::InvalidString(stream) => write!(f, "Invalid string: {stream:?}"),
        }
    }
}

impl std::error::Error for ParseError {
}

fn extract_strings(buffer: &[u8]) -> Vec<String> {
    buffer
        .split(|&c| c == 0)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect()
}

#[derive(Debug, Clone)]
pub enum TftpOption {
    BlockSize(u16),
    Timeout(u8),
    TransferSize(u64),
}

impl TftpOption {
    fn name(&self) -> String {
        match self {
            TftpOption::BlockSize(..) => "blksize",
            TftpOption::Timeout(..) => "timeout",
            TftpOption::TransferSize(..) => "tsize",
        }.into()
    }

    fn encoded_value(&self) -> Vec<u8> {
        match self {
            TftpOption::BlockSize(sz) => sz.to_string(),
            TftpOption::Timeout(tout) => tout.to_string(),
            TftpOption::TransferSize(tsize) => tsize.to_string(),
        }.bytes().collect()
    }

    fn encode(&self) -> Vec<u8> {
        self.name()
            .bytes()
            .chain([0])
            .chain(self.encoded_value().into_iter())
            .chain([0])
            .collect()
    }
}

fn parse_option(name: &str, value: &str) -> Option<TftpOption> {
    match name.to_lowercase().as_str() {
        "blksize" => { // Following RFC 2348
            value.parse::<u16>()
                .ok()
                .filter(|&val| (val > 7 && val < 65465))
                .and_then(|val| Some(TftpOption::BlockSize(val)))
        }
        "timeout" => { // Following RFC 2349
            value.parse::<u8>()
                .ok()
                .filter(|&val| val > 0)
                .and_then(|val| Some(TftpOption::Timeout(val)))
        }
        "tsize" => {  // Following RFC 2394 - does not define upper limit
            value.parse::<u64>()
                .ok()
                .and_then(|val| Some(TftpOption::TransferSize(val)))
        }
        _ => None,
    }
}

struct Arguments {
    filename: String,
    mode: Mode,
    options: Vec<TftpOption>,
}

fn parse_readwrite(buffer: &[u8]) -> Result<Arguments, ParseError> {
    if buffer.len() < 4 {
        return Err(ParseError::CorruptPacket("Too short packet".into()));
    }

    let strings = extract_strings(buffer);

    if strings.len() < 2 {
        Err(ParseError::CorruptPacket("Missing arguments".into()))
    } else {
        let filename = strings[0].clone();
        let possible_mode = &strings[1];
        let mode = match Mode::try_from(possible_mode.as_str()) {
            Ok(mode) => mode,
            Err(_) => return Err(ParseError::InvalidString(possible_mode.into())),
        };
        let options = strings[2..]
            .chunks(2)
            .filter(|chunk| chunk.len() == 2) // To discard leftovers
            .map(|chunk| parse_option(&chunk[0], &chunk[1]))
            .flatten()
            .collect::<Vec<_>>();

        Ok(Arguments {
            filename,
            mode,
            options,
        })
    }
}

pub fn parse_message(buffer: &[u8]) -> Result<Message, ParseError> {
    if buffer.len() < 4 {
        return Err(ParseError::CorruptPacket("Truncated Read/Write packet".into()));
    }

    // Interpret the opcode
    Ok(match u16::from_be_bytes([buffer[0], buffer[1]]) {
        1 => Message::read_from_arguments(parse_readwrite(&buffer[2..])?),
        2 => Message::write_from_arguments(parse_readwrite(&buffer[2..])?),
        3 => { todo!() },
        4 => Message::Ack(u16::from_be_bytes([buffer[2], buffer[3]])),
        5 => { todo!() },
        code => { return Err(ParseError::InvalidOpcode(code)) }
    })
}

#[cfg(test)]
mod tests {
    use crate::{Message, TftpOption};

    #[test]
    fn encode_oack() {
        let options = vec![
            TftpOption::BlockSize(1024),
            TftpOption::TransferSize(100000000),
        ];
        let msg = Message::OptionAck { options };
        eprintln!("{:?}", msg.into_packet());
    }
}

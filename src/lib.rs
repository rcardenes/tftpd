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

impl Into<u16> for PacketType {
    fn into(self) -> u16 {
        match self {
            PacketType::ReadRequest => 1,
            PacketType::WriteRequest => 2,
            PacketType::Data => 3,
            PacketType::Acknowledgement => 4,
            PacketType::Error => 5,
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
}

impl Into<u16> for ErrorCode {
    fn into(self) -> u16 {
        match self {
            ErrorCode::NotDefined => 1,
            ErrorCode::FileNotFound => 2,
            ErrorCode::AccessViolation => 3,
            ErrorCode::DiskFull => 4,
            ErrorCode::IllegalOperation => 5,
            ErrorCode::UnknownTransferId => 6,
            ErrorCode::FileAlreadyExists => 7,
            ErrorCode::NoSuchUser => 8,
        }
    }
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
    Read { filename: String, mode: Mode },
    Write { filename: String, mode: Mode },
    Data { block: u16, payload: Vec<u8> },
    Ack(u16),
    Error { code: ErrorCode, message: String },
}

impl Message {
    pub fn into_packet(self) -> Vec<u8> {
        match self {
            // Message::Read { filename, mode } => todo!(),
            // Message::Write { filename, mode } => todo!(),
            Message::Data { block, payload } => {
                (3 as u16).to_be_bytes().into_iter()
                    .chain((block as u16).to_be_bytes().into_iter())
                    .chain(payload.into_iter())
                    .collect()
            }
            // Message::AckMessage(_) => todo!(),
            Message::Error { code, message } => {
                (5 as u16).to_be_bytes().into_iter()
                    .chain((code as u16).to_be_bytes().into_iter())
                    .chain(message.bytes().into_iter())
                    .chain([0])
                    .collect()
            }
            _ => todo!()
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    CorruptPacket(String),
    InvalidOpcode,
    InvalidString(Vec<u8>),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl std::error::Error for ParseError {
}

fn parse_readwrite(buffer: &[u8]) -> Result<(String, Mode), ParseError> {
    if buffer.len() < 4 {
        return Err(ParseError::CorruptPacket("Too short packet".into()));
    }

    if let Some(p1) = buffer.iter().position(|&b| b == 0) {
        if let Some(p2) = buffer[(p1+1)..].iter().position(|&b| b == 0) {
            let possible_filename = buffer[..p1].to_vec();
            let filename = match String::from_utf8(possible_filename.clone()) {
                Ok(name) => name,
                Err(_) => return Err(ParseError::InvalidString(possible_filename)),
            };

            let beg = p1 + 1;
            let possible_mode = buffer[beg .. (beg + p2)].to_vec();
            let mode = match String::from_utf8(possible_mode.clone()) {
                Ok(name) => {
                    match Mode::try_from(name.as_str()) {
                        Ok(mode) => mode,
                        Err(_) => return Err(ParseError::InvalidString(possible_mode)),
                    }
                },
                Err(_) => return Err(ParseError::InvalidString(possible_mode)),
            };

            Ok((filename, mode))
        } else {
            Err(ParseError::CorruptPacket("Not terminated: mode".into()))
        }
    } else {
        Err(ParseError::CorruptPacket("Not terminated: filename".into()))
    }
}

pub fn parse_message(buffer: &[u8]) -> Result<Message, ParseError> {
    if buffer.len() < 4 {
        return Err(ParseError::CorruptPacket("Truncated Read/Write packet".into()));
    }

    // Interpret the opcode
    Ok(match u16::from_be_bytes([buffer[0], buffer[1]]) {
        1 => {
            let (filename, mode) = parse_readwrite(&buffer[2..])?;
            Message::Read { filename, mode }
        },
        2 => {
            let (filename, mode) = parse_readwrite(&buffer[2..])?;
            Message::Write { filename, mode }
        },
        3 => { todo!() },
        4 => Message::Ack(u16::from_be_bytes([buffer[2], buffer[3]])),
        5 => { todo!() },
        _ => { return Err(ParseError::InvalidOpcode) }
    })
}



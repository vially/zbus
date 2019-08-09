use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::{error, fmt, io};

use nix::unistd::Uid;

use crate::message;
use crate::message_field;
use crate::Variant;
use crate::{VariantError, VariantType};

pub struct Connection {
    pub server_guid: String,

    socket: UnixStream,
    // Serial number for next outgoing message
    serial: u32,
}

#[derive(Debug)]
pub enum ConnectionError {
    IO(io::Error),
    Message(message::MessageError),
    MessageField(message_field::MessageFieldError),
    Variant(VariantError),
    Handshake,
    InvalidReply,
    // According to the spec, there can be all kinds of details in D-Bus errors but nobody adds anything more than a
    // string description.
    MethodError(String, Option<String>),
}

impl error::Error for ConnectionError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ConnectionError::IO(e) => Some(e),
            ConnectionError::Handshake => None,
            ConnectionError::Message(e) => Some(e),
            ConnectionError::MessageField(e) => Some(e),
            ConnectionError::Variant(e) => Some(e),
            ConnectionError::InvalidReply => None,
            ConnectionError::MethodError(_, _) => None,
        }
    }
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConnectionError::IO(e) => write!(f, "I/O error: {}", e),
            ConnectionError::Handshake => write!(f, "D-Bus handshake failed"),
            ConnectionError::Message(e) => write!(f, "Message creation error: {}", e),
            ConnectionError::MessageField(e) => write!(f, "Message field parsing error: {}", e),
            ConnectionError::Variant(e) => write!(f, "{}", e),
            ConnectionError::InvalidReply => write!(f, "Invalid D-Bus method reply"),
            ConnectionError::MethodError(name, detail) => write!(
                f,
                "{}: {}",
                name,
                detail.as_ref().map(|s| s.as_str()).unwrap_or("no details")
            ),
        }
    }
}

impl From<io::Error> for ConnectionError {
    fn from(val: io::Error) -> Self {
        ConnectionError::IO(val)
    }
}

impl From<message::MessageError> for ConnectionError {
    fn from(val: message::MessageError) -> Self {
        ConnectionError::Message(val)
    }
}

impl From<message_field::MessageFieldError> for ConnectionError {
    fn from(val: message_field::MessageFieldError) -> Self {
        ConnectionError::MessageField(val)
    }
}

impl From<VariantError> for ConnectionError {
    fn from(val: VariantError) -> Self {
        ConnectionError::Variant(val)
    }
}

// For messages that are D-Bus error returns
impl From<message::Message> for ConnectionError {
    fn from(message: message::Message) -> ConnectionError {
        // FIXME: Instead of checking this, we should have Method as trait and specific types for
        // each message type.
        if message.message_type() != message::MessageType::Error {
            return ConnectionError::InvalidReply;
        }

        match message.fields() {
            Ok(all_fields) => {
                // First, get the error name
                let name = match all_fields
                    .iter()
                    .find(|f| f.code() == message_field::MessageFieldCode::ErrorName)
                {
                    Some(f) => match f.value() {
                        Ok(v) => match v.get::<(&str)>() {
                            Ok(s) => String::from(s),
                            Err(e) => return ConnectionError::Variant(e),
                        },
                        Err(e) => return ConnectionError::MessageField(e),
                    },
                    None => return ConnectionError::InvalidReply,
                };

                // Then, try to get the optional description string
                if message
                    .body_signature()
                    .map(|s| s.as_str() == <(&str)>::SIGNATURE_STR)
                    .unwrap_or(false)
                {
                    match message.body() {
                        Ok(body) => match Variant::from_data(&body, "s") {
                            Ok(v) => match v.get::<(&str)>() {
                                Ok(detail) => {
                                    ConnectionError::MethodError(name, Some(String::from(detail)))
                                }
                                Err(e) => return ConnectionError::Variant(e),
                            },
                            Err(e) => ConnectionError::Variant(e),
                        },
                        Err(e) => ConnectionError::Message(e),
                    }
                } else {
                    ConnectionError::MethodError(name, None)
                }
            }
            Err(e) => return ConnectionError::Message(e),
        }
    }
}

impl Connection {
    pub fn new_session() -> Result<Self, ConnectionError> {
        // FIXME: Currently just assume a path
        let uid = Uid::current();
        let path = format!("/run/user/{}/bus", uid);
        let mut socket = UnixStream::connect(path)?;

        // SASL Handshake
        let uid_str = uid
            .to_string()
            .chars()
            .map(|c| format!("{:x}", c as u32))
            .collect::<String>();
        socket.write(format!("\0AUTH EXTERNAL {}\r\n", uid_str).as_bytes())?;
        let mut buf_reader = BufReader::new(&socket);
        let mut buf = String::new();
        let bytes_read = buf_reader.read_line(&mut buf)?;
        let mut components = buf.split_whitespace();
        if bytes_read < 3 || components.next() != Some("OK") {
            return Err(ConnectionError::Handshake);
        }

        let server_guid = String::from(components.next().ok_or(ConnectionError::Handshake)?);

        socket.write(b"BEGIN\r\n")?;

        let mut connection = Self {
            socket,
            server_guid,
            serial: 0,
        };

        // Now that daemon has approved us, we must send a hello as per specs
        let reply = connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "Hello",
            None,
        )?;

        if reply
            .body_signature()
            .map(|s| s.as_str() == <(&str)>::SIGNATURE_STR)
            .unwrap_or(false)
        {
            let body = reply.body()?;
            let v = Variant::from_data(&body, "s")?;
            let bus_name = v.get::<(&str)>()?;

            println!("bus name: {}", bus_name);
        } else {
            return Err(ConnectionError::InvalidReply);
        }

        Ok(connection)
    }

    pub fn call_method(
        &mut self,
        destination: Option<&str>,
        path: &str,
        iface: Option<&str>,
        method_name: &str,
        body: Option<Variant>,
    ) -> Result<message::Message, ConnectionError> {
        println!("Starting: {}", method_name);
        let serial = self.next_serial();
        let m = message::Message::method(destination, path, iface, method_name, body)?
            .set_serial(serial);

        self.socket.write(m.as_bytes())?;

        loop {
            // FIXME: We need to read incoming messages in a separate thread and maintain a queue

            let mut buf = [0; message::PRIMARY_HEADER_SIZE];
            self.socket.read(&mut buf[..])?;

            let mut incoming = message::Message::from_bytes(&buf)?;
            let bytes_left = incoming.bytes_to_completion();
            if bytes_left == 0 {
                return Err(ConnectionError::Handshake);
            }
            let mut buf = vec![0; bytes_left as usize];
            let bytes_read = self.socket.read(&mut buf[..])?;
            incoming.add_bytes(&buf[0..bytes_read])?;

            if incoming.message_type() == message::MessageType::MethodReturn
                || incoming.message_type() == message::MessageType::Error
            {
                let all_fields = incoming.fields()?;

                if all_fields
                    .iter()
                    .find(|f| {
                        f.code() == message_field::MessageFieldCode::ReplySerial
                            && f.value()
                                .map(|v| v.get::<u32>().map(|u| u == serial).unwrap_or(false))
                                .unwrap_or(false)
                    })
                    .is_some()
                {
                    match incoming.message_type() {
                        message::MessageType::Error => return Err(incoming.into()),
                        message::MessageType::MethodReturn => {
                            println!("Returing from: {}", method_name);
                            return Ok(incoming);
                        }
                        _ => (),
                    }
                }
            }
        }
    }

    fn next_serial(&mut self) -> u32 {
        self.serial += 1;

        self.serial
    }
}

use super::*;
use crate::languageclient::LanguageClient;
use crate::vim;

#[derive(Debug)]
pub enum RawMessage {
    MethodCall(String, Value),
    Notification(String, Value),
    Output(Id, Fallible<Value>),
}

impl actix::Message for RawMessage {
    type Result = Fallible<Value>;
}

pub struct RpcClient {
    languageId: LanguageId,
    id: Id,
    writer: Box<Write>,
    rx: Receiver<rpc::Output>,
}

impl RpcClient {
    pub fn new(
        languageId: LanguageId,
        reader: impl BufRead + Send + 'static,
        writer: impl Write + 'static,
        language_client: actix::Addr<LanguageClient>,
    ) -> Fallible<Self> {
        let (tx, rx) = channel();

        let languageId_clone = languageId.clone();
        thread::Builder::new()
            .name(format!("reader-{:?}", languageId))
            .spawn(move || {
                let languageId = languageId_clone;

                // Count how many consequent empty lines.
                let mut count_empty_lines = 0;

                let mut reader = reader;
                let mut content_length = 0;
                loop {
                    let mut message = String::new();
                    let mut line = String::new();
                    if languageId.is_some() {
                        reader.read_line(&mut line)?;
                        let line = line.trim();
                        if line.is_empty() {
                            count_empty_lines += 1;
                            if count_empty_lines > 5 {
                                bail!("Unable to read from language server");
                            }

                            let mut buf = vec![0; content_length];
                            reader.read_exact(buf.as_mut_slice())?;
                            message = String::from_utf8(buf)?;
                        } else {
                            count_empty_lines = 0;
                            if !line.starts_with("Content-Length") {
                                continue;
                            }

                            let tokens: Vec<&str> = line.splitn(2, ':').collect();
                            let len = tokens
                                .get(1)
                                .ok_or_else(|| {
                                    format_err!("Failed to get length! tokens: {:?}", tokens)
                                })?.trim();
                            content_length = usize::from_str(len)?;
                        }
                    } else if reader.read_line(&mut message)? == 0 {
                        break;
                    }

                    let message = message.trim();
                    if message.is_empty() {
                        continue;
                    }
                    info!("<= {:?} {}", languageId, message);
                    // FIXME: Remove extra `meta` property from javascript-typescript-langserver.
                    let s = message.replace(r#","meta":{}"#, "");
                    let message = serde_json::from_str(&s);
                    if let Err(ref err) = message {
                        error!(
                            "Failed to deserialize output: {}\n\n Message: {}\n\nError: {:?}",
                            err, s, err
                        );
                        continue;
                    }
                    // TODO: cleanup.
                    let message = message.unwrap();
                    match message {
                        vim::RawMessage::MethodCall(method_call) => {
                            language_client
                                .do_send(Call::MethodCall(languageId.clone(), method_call));
                        }
                        vim::RawMessage::Notification(notification) => {
                            language_client
                                .do_send(Call::Notification(languageId.clone(), notification));
                        }
                        vim::RawMessage::Output(output) => {
                            tx.send(output)?;
                        }
                    };
                }

                Ok(())
            })?;

        Ok(RpcClient {
            languageId,
            id: 0,
            writer: Box::new(writer),
            rx,
        })
    }

    fn write(&mut self, message: &impl Serialize) -> Fallible<()> {
        let s = serde_json::to_string(message)?;
        info!("=> {:?} {}", self.languageId, s);
        write!(self.writer, "Content-Length: {}\r\n\r\n{}", s.len(), s)?;
        Ok(())
    }
}

impl actix::Actor for RpcClient {
    type Context = actix::Context<Self>;
}

impl actix::Handler<RawMessage> for RpcClient {
    type Result = Fallible<Value>;

    fn handle(&mut self, msg: RawMessage, ctx: &mut actix::Context<Self>) -> Self::Result {
        let msg_rpc = match &msg {
            RawMessage::MethodCall(method, params) => {
                self.id += 1;
                vim::RawMessage::MethodCall(rpc::MethodCall {
                    jsonrpc: Some(rpc::Version::V2),
                    id: rpc::Id::Num(self.id),
                    method: serde_json::to_string(method)?,
                    params: params.to_params()?,
                })
            }
            RawMessage::Notification(method, params) => {
                vim::RawMessage::Notification(rpc::Notification {
                    jsonrpc: Some(rpc::Version::V2),
                    method: serde_json::to_string(method)?,
                    params: params.to_params()?,
                })
            }
            RawMessage::Output(id, result) => match result {
                Ok(ok) => vim::RawMessage::Output(rpc::Output::Success(rpc::Success {
                    jsonrpc: Some(rpc::Version::V2),
                    id: rpc::Id::Num(*id),
                    result: serde_json::to_value(ok)?,
                })),
                Err(err) => vim::RawMessage::Output(rpc::Output::Failure(rpc::Failure {
                    jsonrpc: Some(rpc::Version::V2),
                    id: rpc::Id::Num(*id),
                    error: err.to_rpc_error(),
                })),
            },
        };

        self.write(&msg_rpc)?;

        if let RawMessage::MethodCall(_, _) = &msg {
            loop {
                let output = self.rx.recv()?;
                if output.id() != &rpc::Id::Num(self.id) {
                    error!("Unexpcted output: {:?}", output);
                    continue;
                }

                match self.rx.recv()? {
                    rpc::Output::Success(success) => return Ok(success.result),
                    rpc::Output::Failure(failure) => {
                        return Err(format_err!("{}", failure.error.message))
                    }
                }
            }
        } else {
            Ok(Value::Null)
        }
    }
}

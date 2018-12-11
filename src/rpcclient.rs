use super::*;
use crate::languageclient::LanguageClient;
use crate::vim;
use crate::types::{Call, Lock};
use futures::sync::oneshot;
use std::sync::mpsc;

#[derive(Clone)]
pub struct RpcClient {
    languageId: LanguageId,
    id: Arc<Mutex<Id>>,
    writer: Arc<Mutex<Write>>,
    tx: mpsc::Sender<(Id, oneshot::Sender<rpc::Output>)>,
}

impl RpcClient {
    pub fn new(
        languageId: LanguageId,
        reader: impl BufRead + Send + 'static,
        writer: impl Write + 'static,
        sink: mpsc::Sender<rpc::Call>,
    ) -> Fallible<Self> {
        let (tx, rx) = channel();

        let languageId_clone = languageId.clone();
        thread::Builder::new()
            .name(format!("reader-{:?}", languageId))
            .spawn(move || {
                let languageId = languageId_clone;
                let pending_outputs = HashMap::new();

                // Count how many consequent empty lines.
                let mut count_empty_lines = 0;

                let mut reader = reader;
                let mut content_length = 0;
                loop {
                    match rx.try_recv() {
                        Ok((id, tx)) => {
                            pending_outputs.insert(id, tx);
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            break;
                        },
                        Err(mpsc::TryRecvError::Empty) => {}
                    }

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
                            sink.send(Call::MethodCall(languageId.clone(), method_call))?;
                        }
                        vim::RawMessage::Notification(notification) => {
                            sink.send(Call::Notification(languageId.clone(), notification))?;
                        }
                        vim::RawMessage::Output(output) => {
                            if let Some(tx) = pending_outputs.remove(output.id().to_int()?) {
                                tx.send(output)?;
                            }
                        }
                    };
                }

                info!("reader-{:?} terminated", languageId);
                Ok(())
            })?;

        Ok(RpcClient {
            languageId,
            id: Arc::new(Mutex::new(0)),
            writer: Arc::new(Mutex::new(writer)),
            tx,
        })
    }

    fn write(&self, message: &impl Serialize) -> Fallible<()> {
        let s = serde_json::to_string(message)?;
        info!("=> {:?} {}", self.languageId, s);
        write!(self.writer.lock()?, "Content-Length: {}\r\n\r\n{}", s.len(), s)?;
        Ok(())
    }

    fn call<R: DeserializeOwned>(&self, method: &impl AsRef<str>, params: &impl Serialize) -> Fallible<R> {
        let method = method.as_ref();
        let id = {
            id = self.id.lock()?;
            id += 1;
            id
        };
        let msg = rpc::MethodCall {
            jsonrpc: Some(rpc::Version::V2),
            id: rpc::Id::Num(id),
            method: method.to_owned(),
            params: params.to_params()?,
        };
        let (tx, rx) = oneshot::channel();
        self.tx.send((id, tx))?;
        self.write(&msg)?;
        match rx.wait()? {
            rpc::Output::Success(ok) => Ok(serde_json::from_value(ok.result)?),
            rpc::Output::Failure(err) => bail!("Error: {:?}", err),
        }
    }

    fn notify(&self, method: &impl AsRef<str>, params: &impl Serialize) -> Fallible<()> {
        let method = method.as_ref();

        let msg = rpc::Notification {
            jsonrpc: Some(rpc::Version::V2),
            method: method.to_owned(),
            params: params.to_params()?,
        };
        self.write(&msg)
    }

    fn output(&self, id: &Id, result: Fallible<impl Serialize>) -> Fallible<()> {
        let output = match result {
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
        };

        self.write(&output)
    }
}

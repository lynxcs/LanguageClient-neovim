use super::*;
use crate::languageclient::StateExt;

pub struct LanguageClientWorker(pub Arc<Mutex<State>>);

impl actix::Actor for LanguageClientWorker {
    type Context = actix::Context<Self>;
}

impl actix::Handler<Call> for LanguageClientWorker {
    type Result = Fallible<()>;

    fn handle(&mut self, msg: Call, ctx: &mut actix::Context<Self>) -> Self::Result {
        match msg {
            Call::MethodCall(lang_id, method_call) => {
                let result = self.handle_method_call(lang_id.as_deref(), &method_call);
                if let Err(ref err) = result {
                    error!("{:?}", err);
                    if err.find_root_cause().downcast_ref::<LCError>().is_none() {
                        error!(
                            "Error handling message: {}\n\nMessage: {}\n\nError: {:?}",
                            err,
                            serde_json::to_string(&method_call).unwrap_or_default(),
                            err
                        );
                    }
                }
                let _ = self.output(lang_id, method_call.id.to_int()?, result);
            }
            Call::Notification(lang_id, notification) => {
                let result = self.handle_notification(lang_id.as_deref(), &notification);
                if let Err(ref err) = result {
                    if err.downcast_ref::<LCError>().is_none() {
                        error!(
                            "Error handling message: {}\n\nMessage: {}\n\nError: {:?}",
                            err,
                            serde_json::to_string(&notification).unwrap_or_default(),
                            err
                        );
                    }
                }
            }
        }

        // TODO
        if let Err(err) = self.handle_fs_events() {
            warn!("{:?}", err);
        }

        ctx.stop();
        Ok(())
    }
}

impl LanguageClientWorker {
    pub fn lock(&self) -> Fallible<MutexGuard<State>> {
        self.0.lock()
    }

    pub fn get<T>(&self, f: impl FnOnce(&State) -> Fallible<T>) -> Fallible<T> {
        f(self.lock()?.deref())
    }

    pub fn update<T>(&self, f: impl FnOnce(&mut State) -> Fallible<T>) -> Fallible<T> {
        let mut state = self.lock()?;
        let mut state = state.deref_mut();

        let v = if log_enabled!(log::Level::Debug) {
            let s = serde_json::to_string(&state)?;
            serde_json::from_str(&s)?
        } else {
            Value::default()
        };

        let result = f(&mut state);

        let next_v = if log_enabled!(log::Level::Debug) {
            let s = serde_json::to_string(&state)?;
            serde_json::from_str(&s)?
        } else {
            Value::default()
        };

        for (k, (v1, v2)) in diff_value(&v, &next_v, "state") {
            debug!("{}: {} ==> {}", k, v1, v2);
        }
        result
    }

    pub fn call<S: Serialize, T: DeserializeOwned>(
        &self,
        languageId: LanguageId,
        method: &str,
        params: S,
    ) -> Fallible<T> {
        let addr = self
            .lock()?
            .addrs
            .get(&languageId)
            .cloned()
            .ok_or_else(|| format_err!("Failed to get RpcClient addr for {:?}", languageId))?;

        info!("addr: {}", addr.connected());

        let v = addr
            .send(rpcclient::RawMessage::MethodCall(
                method.to_owned(),
                serde_json::to_value(params)?,
            )).wait()??;
        info!("{:?}", v);
        Ok(serde_json::from_value(v)?)
    }

    pub fn notify(
        &self,
        languageId: LanguageId,
        method: &str,
        params: impl Serialize,
    ) -> Fallible<()> {
        let addr = self
            .lock()?
            .addrs
            .get(&languageId)
            .cloned()
            .ok_or_else(|| format_err!("Failed to get RpcClient addr for {:?}", languageId))?;

        addr.do_send(rpcclient::RawMessage::Notification(
            method.to_owned(),
            serde_json::to_value(params)?,
        ));
        Ok(())
    }

    pub fn output(
        &self,
        languageId: LanguageId,
        id: Id,
        result: Fallible<impl Serialize>,
    ) -> Fallible<()> {
        let addr = self
            .lock()?
            .addrs
            .get(&languageId)
            .cloned()
            .ok_or_else(|| format_err!("Failed to get RpcClient addr for {:?}", languageId))?;

        let result = result.and_then(|r| Ok(serde_json::to_value(r)?));
        addr.do_send(rpcclient::RawMessage::Output(id, result));
        Ok(())
    }
}

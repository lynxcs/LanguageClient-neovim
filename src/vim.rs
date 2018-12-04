use super::*;
use crate::languageclientworker::LanguageClientWorker;

impl LanguageClientWorker {
    /////// Vim wrappers ///////

    #[allow(needless_pass_by_value)]
    pub fn eval<E, T>(&self, exp: E) -> Fallible<T>
    where
        E: VimExp,
        T: DeserializeOwned,
    {
        let result = self.call(None, "eval", exp.to_exp())?;
        Ok(serde_json::from_value(result)?)
    }

    pub fn command<P: Serialize + Debug>(&self, cmds: P) -> Fallible<()> {
        if self.call::<_, u8>(None, "execute", &cmds)? != 0 {
            bail!("Failed to execute command: {:?}", cmds);
        }
        Ok(())
    }

    ////// Vim builtin function wrappers ///////

    pub fn echo(&self, message: impl AsRef<str>) -> Fallible<()> {
        self.notify(None, "s:Echo", message.as_ref())
    }

    pub fn echo_ellipsis(&self, message: impl AsRef<str>) -> Fallible<()> {
        let message = message.as_ref().lines().collect::<Vec<_>>().join(" ");
        self.notify(None, "s:EchoEllipsis", message)
    }

    pub fn echomsg_ellipsis(&self, message: impl AsRef<str>) -> Fallible<()> {
        let message = message.as_ref().lines().collect::<Vec<_>>().join(" ");
        self.notify(None, "s:EchomsgEllipsis", message)
    }

    pub fn echomsg(&self, message: impl AsRef<str>) -> Fallible<()> {
        self.notify(None, "s:Echomsg", message.as_ref())
    }

    pub fn echoerr(&self, message: impl AsRef<str>) -> Fallible<()> {
        self.notify(None, "s:Echoerr", message.as_ref())
    }

    pub fn echowarn(&self, message: impl AsRef<str>) -> Fallible<()> {
        self.notify(None, "s:Echowarn", message.as_ref())
    }

    pub fn cursor(&self, lnum: u64, col: u64) -> Fallible<()> {
        self.notify(None, "cursor", json!([lnum, col]))
    }

    pub fn setline(&self, lnum: u64, text: &[String]) -> Fallible<()> {
        if self.call::<_, u8>(None, "setline", json!([lnum, text]))? != 0 {
            bail!("Failed to set buffer content!");
        }
        Ok(())
    }

    pub fn edit(&self, goto_cmd: &Option<String>, path: impl AsRef<Path>) -> Fallible<()> {
        let path = path.as_ref().to_string_lossy();

        let goto = goto_cmd.as_deref().unwrap_or("edit");
        if self.call::<_, u8>(None, "s:Edit", json!([goto, path]))? != 0 {
            bail!("Failed to edit file: {}", path);
        }

        if path.starts_with("jdt://") {
            self.command("setlocal buftype=nofile filetype=java noswapfile")?;

            let result = self.java_classFileContents(&json!({
                VimVar::LanguageId.to_key(): "java",
                "uri": path,
            }))?;
            let content = match result {
                Value::String(s) => s,
                _ => bail!("Unexpected type: {:?}", result),
            };
            let lines: Vec<String> = content
                .lines()
                .map(std::string::ToString::to_string)
                .collect();
            self.setline(1, &lines)?;
        }
        Ok(())
    }

    pub fn setqflist(&self, list: &[QuickfixEntry], action: &str, title: &str) -> Fallible<()> {
        let parms = json!([list, action]);
        if self.call::<_, u8>(None, "setqflist", parms)? != 0 {
            bail!("Failed to set quickfix list!");
        }
        let parms = json!([[], "a", { "title": title }]);
        if self.call::<_, u8>(None, "setqflist", parms)? != 0 {
            bail!("Failed to set quickfix list title!");
        }
        Ok(())
    }

    pub fn setloclist(&self, list: &[QuickfixEntry], action: &str, title: &str) -> Fallible<()> {
        let parms = json!([0, list, action]);
        if self.call::<_, u8>(None, "setloclist", parms)? != 0 {
            bail!("Failed to set location list!");
        }
        let parms = json!([0, [], "a", { "title": title }]);
        if self.call::<_, u8>(None, "setloclist", parms)? != 0 {
            bail!("Failed to set location list title!");
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawMessage {
    Notification(rpc::Notification),
    MethodCall(rpc::MethodCall),
    Output(rpc::Output),
}

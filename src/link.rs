//! The editor from outside: what the command line asks for, the one window a file is open
//! in, and a command run after each save.
//!
//! `palaver [FILE] [--flow NAME] [--node ALIAS|ID] [--after-save COMMAND]
//! [--declarations FILE]` opens FILE, shows the flow and selects the node, so another tool
//! (a game's level editor, a script) can open a conversation at the line it is about. `--node` takes what [`Doc::find_node`]
//! takes: an alias, or an id, never a position.
//!
//! A window listens on a socket for its current file (`temp_dir()/palaver-<hash>.sock`,
//! Unix only). A later launch for that file hands its flow, node, declarations and
//! after-save command to the window and exits. Switching documents releases the old socket
//! so another window can take over. A socket left by a killed window is replaced on the next
//! launch. A window that opens or saves as a file another window holds asks first
//! ([`Handovers::open_elsewhere`]) and leaves it to that window, so one file is never saved
//! from two.
//!
//! **`--after-save` is a shell command run after each successful save**, in the file's
//! folder, with `PALAVER_FILE` set to its path: the build step a file feeds (a converter, an
//! exporter) that would otherwise be forgotten. Hooks run in save order on a background
//! thread, and the status bar reports each result. Handed over, it replaces the window's.
//!
//! **`--declarations` names a JSON file of variables and characters** the conversation may
//! use without declaring them ([`Declarations`]): the tool's own names, a game's quests and
//! people, shown in the panes as declared from outside, locked, and never written into the
//! `.dln`. A later launch hands its file over with its place, so a window already open
//! picks up names the tool has added since.
//!
//! **`--export-ftl`, `--export-screenplay` and `--export-recording` write those documents
//! from FILE and open no window** — a build step, typically the `--after-save` command
//! itself: `palaver "$PALAVER_FILE" --export-ftl strings.ftl`. They run before the hand-over,
//! or an export launched while the file was open would hand itself to the window and write
//! nothing, and before gpui starts, so a build machine with no display can run them. `-` is
//! standard output. The flags that are only about a window — `--flow`, `--node`,
//! `--after-save`, `--declarations` — are refused beside them rather than ignored.
//!
//! [`Doc::find_node`]: crate::doc::Doc::find_node
//! [`Declarations`]: crate::doc::Declarations
use futures_channel::oneshot;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;

use crate::doc::{Doc, Problem, replacement_target};

pub const USAGE: &str = "usage: palaver [FILE] [--flow NAME] [--node ALIAS|ID] \
     [--after-save COMMAND] [--declarations FILE]
       palaver FILE [--export-ftl PATH] [--export-screenplay PATH] [--export-recording PATH]
         writes the exports and opens no window; PATH - is standard output";

/// One of the documents the File menu exports, as the command line names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    Ftl,
    Screenplay,
    Recording,
}

impl Export {
    const ALL: [Export; 3] = [Export::Ftl, Export::Screenplay, Export::Recording];

    fn flag(self) -> &'static str {
        match self {
            Export::Ftl => "--export-ftl",
            Export::Screenplay => "--export-screenplay",
            Export::Recording => "--export-recording",
        }
    }

    /// The same text the File menu writes, and through the same `Doc` function for a file.
    fn text(self, doc: &Doc) -> String {
        match self {
            Export::Ftl => doc.to_ftl(),
            Export::Screenplay => doc.to_screenplay(),
            Export::Recording => doc.to_recording_script(),
        }
    }

    fn write(self, doc: &Doc, path: &Path) -> Result<(), String> {
        match self {
            Export::Ftl => doc.export_ftl(path),
            Export::Screenplay => doc.export_screenplay(path),
            Export::Recording => doc.export_recording_script(path),
        }
    }
}

/// What the command line asked for.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub file: Option<PathBuf>,
    pub flow: Option<String>,
    pub node: Option<String>,
    pub after_save: Option<String>,
    pub declarations: Option<PathBuf>,
    /// Each document to write and where, in the order asked; `-` is standard output. Not
    /// empty means no window opens.
    pub exports: Vec<(Export, PathBuf)>,
}

/// Reads the arguments after the program's name. `Err("")` is a request for the usage
/// line; any other `Err` says what was wrong with them.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Request, String> {
    let mut request = Request::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        let mut text = |name| {
            value(name)?
                .into_string()
                .map_err(|_| format!("{name} needs a UTF-8 value"))
        };
        match arg.to_str() {
            Some("--flow") => request.flow = Some(text("--flow")?),
            Some("--node") => request.node = Some(text("--node")?),
            Some("--after-save") => request.after_save = Some(text("--after-save")?),
            Some("--declarations") => {
                request.declarations = Some(PathBuf::from(value("--declarations")?));
            }
            Some(flag) if Export::ALL.iter().any(|export| export.flag() == flag) => {
                let export = Export::ALL.into_iter().find(|e| e.flag() == flag).unwrap();
                if request.exports.iter().any(|(given, _)| *given == export) {
                    return Err(format!("{flag} given twice"));
                }
                let path = value(flag)?;
                if path.as_encoded_bytes().starts_with(b"--") || path == "-h" {
                    return Err(format!("{flag} needs a value"));
                }
                request.exports.push((export, path.into()));
            }
            Some("-h" | "--help") => return Err(String::new()),
            _ if arg.as_encoded_bytes().starts_with(b"--") => {
                return Err(format!("unknown option {}", arg.to_string_lossy()));
            }
            _ => {
                if request.file.is_some() {
                    return Err("one file at a time".into());
                }
                request.file = Some(PathBuf::from(arg));
            }
        }
    }
    if !request.exports.is_empty() {
        if request.file.is_none() {
            return Err("an export needs the FILE to export".into());
        }
        let window_only = [
            ("--flow", request.flow.is_some()),
            ("--node", request.node.is_some()),
            ("--after-save", request.after_save.is_some()),
            ("--declarations", request.declarations.is_some()),
        ];
        if let Some((flag, _)) = window_only.iter().find(|(_, given)| *given) {
            return Err(format!(
                "{flag} is for the window, and an export opens none"
            ));
        }
        if request
            .exports
            .iter()
            .filter(|(_, path)| path.as_os_str() == "-")
            .count()
            > 1
        {
            return Err("only one export can go to standard output (-)".into());
        }
    }
    Ok(request)
}

/// Protects the source document when an export names the same path, including through a
/// symlink. Uses the same resolution as the writer.
pub fn check_export_destination(source: &Path, destination: &Path) -> Result<(), String> {
    if let (Ok(source), Ok(destination)) =
        (replacement_target(source), replacement_target(destination))
        && source == destination
    {
        return Err("export destination is the source document".into());
    }
    Ok(())
}

/// Writes every export `request` asks for from its FILE as saved on disk, in the order
/// asked, and opens no window. `Ok` carries what is worth saying on stderr even when it
/// worked — lines still marked as placeholders, which the File menu asks about and a build
/// script cannot be asked — and `Err` the first thing that failed; an export already written
/// stays written. Standard output is `out`, so a test can read it.
pub fn export(request: &Request, out: &mut impl Write) -> Result<Vec<String>, String> {
    let file = request
        .file
        .as_deref()
        .ok_or("an export needs the FILE to export")?;
    let doc = Doc::load(file).map_err(|why| format!("{}: {why}", file.display()))?;
    // Every destination is checked before any is written, so a refusal writes nothing.
    let mut written: Vec<(Export, PathBuf)> = Vec::new();
    for (export, path) in request.exports.iter().filter(|(_, path)| path != "-") {
        check_export_destination(file, path).map_err(|why| format!("{}: {why}", path.display()))?;
        // Two exports to one file would leave only the second, and say it worked.
        let target = replacement_target(path)
            .unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
        if let Some((first, _)) = written.iter().find(|(_, other)| *other == target) {
            return Err(format!(
                "{}: {} and {} both write it",
                path.display(),
                first.flag(),
                export.flag()
            ));
        }
        written.push((*export, target));
    }
    for (export, path) in &request.exports {
        if path.as_os_str() == "-" {
            out.write_all(export.text(&doc).as_bytes())
                .map_err(|why| format!("standard output: {why}"))?;
        } else {
            export
                .write(&doc, path)
                .map_err(|why| format!("{}: {why}", path.display()))?;
        }
    }
    let placeholders = doc
        .problems()
        .iter()
        .filter(|p| matches!(p, Problem::Placeholder(_) | Problem::PlaceholderChoice(_)))
        .count();
    Ok(match placeholders {
        0 => Vec::new(),
        1 => vec!["1 line is still a placeholder; it is marked in the export".into()],
        n => vec![format!(
            "{n} lines are still placeholders; they are marked in the export"
        )],
    })
}

/// How an after-save command went: whether it succeeded, and a line to say about it.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub detail: String,
}

struct SaveHook {
    command: String,
    file: PathBuf,
    reply: oneshot::Sender<Outcome>,
}

/// Runs an editor's hooks in save order, even when an earlier hook is still running.
#[derive(Default)]
pub struct AfterSaveQueue {
    jobs: Option<mpsc::Sender<SaveHook>>,
}

impl AfterSaveQueue {
    pub fn enqueue(
        &mut self,
        command: String,
        file: PathBuf,
    ) -> io::Result<oneshot::Receiver<Outcome>> {
        if self.jobs.is_none() {
            let (jobs, pending) = mpsc::channel::<SaveHook>();
            std::thread::Builder::new().spawn(move || {
                for job in pending {
                    let outcome = run_after_save(&job.command, &job.file);
                    let _ = job.reply.send(outcome);
                }
            })?;
            self.jobs = Some(jobs);
        }
        let (reply, result) = oneshot::channel();
        self.jobs
            .as_ref()
            .unwrap()
            .send(SaveHook {
                command,
                file,
                reply,
            })
            .map_err(|_| io::Error::other("after-save worker stopped"))?;
        Ok(result)
    }
}

/// Runs `command` through the shell in `file`'s folder, with `PALAVER_FILE` set. Blocking:
/// the caller runs it off the foreground. The line said about a failure is the last one
/// the command wrote to stderr, else to stdout, since that is where a tool says why.
fn run_after_save(command: &str, file: &Path) -> Outcome {
    #[cfg(unix)]
    let mut shell = {
        let mut shell = Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    };
    #[cfg(windows)]
    let mut shell = {
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    };
    // Absolute before the folder changes under it: `sub/talk.dln`, read from inside `sub`,
    // names `sub/sub/talk.dln`. Not canonicalized — a symlink stays the name it was given.
    let file = &std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    if let Some(folder) = file
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        shell.current_dir(folder);
    }
    shell.env("PALAVER_FILE", file);
    match shell.output() {
        Ok(output) if output.status.success() => Outcome {
            ok: true,
            detail: String::new(),
        },
        Ok(output) => {
            let last = |bytes: &[u8]| {
                String::from_utf8_lossy(bytes)
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
            };
            let said = last(&output.stderr).or_else(|| last(&output.stdout));
            let code = output
                .status
                .code()
                .map_or("a signal".to_string(), |code| format!("exit {code}"));
            Outcome {
                ok: false,
                detail: match said {
                    Some(line) => format!("{code}: {line}"),
                    None => code,
                },
            }
        }
        Err(error) => Outcome {
            ok: false,
            detail: error.to_string(),
        },
    }
}

#[cfg(unix)]
pub use socket::{Handovers, hand_over};

#[cfg(unix)]
mod socket {
    use std::ffi::OsStr;
    use std::hash::{Hash, Hasher};
    use std::io::{self, BufRead, BufReader, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::Duration;

    /// How long either side waits for the other before giving up.
    const PATIENCE: Duration = Duration::from_secs(3);

    /// A request handed over from a later launch: where to go, and where to answer
    /// whether this window took it.
    pub struct Goto {
        pub flow: String,
        pub node: String,
        /// A declarations file to read again.
        pub declarations: Option<PathBuf>,
        /// An after-save command to run from now on, in place of the window's own.
        pub after_save: Option<String>,
        pub reply: mpsc::Sender<bool>,
    }

    /// Another window is listening for the file: it has it open.
    #[derive(Debug)]
    pub struct Taken;

    /// The socket for a file: one per file, so two files open in two windows each have
    /// their own. Named by a hash of the canonical path, which is short, the same for
    /// every way of spelling the path, and safe as a file name.
    fn path_for(file: &Path) -> PathBuf {
        let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut hasher);
        std::env::temp_dir().join(format!("palaver-{:016x}.sock", hasher.finish()))
    }

    /// A field of the one-line protocol, escaped: tabs and newlines are its separators, and a
    /// shell command or a path may hold either and must arrive as it was sent.
    fn escape(field: &[u8], line: &mut Vec<u8>) {
        for &byte in field {
            match byte {
                b'\\' => line.extend_from_slice(b"\\\\"),
                b'\t' => line.extend_from_slice(b"\\t"),
                b'\n' => line.extend_from_slice(b"\\n"),
                b'\r' => line.extend_from_slice(b"\\r"),
                _ => line.push(byte),
            }
        }
    }

    fn unescape(field: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(field.len());
        let mut bytes = field.iter();
        while let Some(&byte) = bytes.next() {
            out.push(match (byte, bytes.as_slice().first()) {
                (b'\\', Some(&next)) => {
                    bytes.next();
                    match next {
                        b't' => b'\t',
                        b'n' => b'\n',
                        b'r' => b'\r',
                        other => other,
                    }
                }
                _ => byte,
            });
        }
        out
    }

    /// Hands `flow`, `node`, a declarations file and an after-save command to the window
    /// that has `file` open. True when that window took them; false when there is none, or
    /// it has another file open now. Each rides as a field of its own, which a window from
    /// before it reads past.
    pub fn hand_over(
        file: &Path,
        flow: &str,
        node: &str,
        declarations: Option<&Path>,
        after_save: Option<&str>,
    ) -> bool {
        let Ok(mut stream) = UnixStream::connect(path_for(file)) else {
            return false;
        };
        let _ = stream.set_read_timeout(Some(PATIENCE));
        let mut line = b"goto".to_vec();
        for field in [
            flow.as_bytes(),
            node.as_bytes(),
            declarations.map_or(&[][..], |path| path.as_os_str().as_bytes()),
            after_save.unwrap_or_default().as_bytes(),
        ] {
            line.push(b'\t');
            escape(field, &mut line);
        }
        line.push(b'\n');
        if stream.write_all(&line).is_err() {
            return false;
        }
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim() == "ok"
    }

    /// The listener follows the document currently owned by the editor.
    #[derive(Default)]
    pub struct Handovers(Option<Listener>);

    impl Handovers {
        /// Listens for `file` from now on, and for nothing else. `Err` when another window
        /// already listens for it; any other failure to listen costs only the hand-over.
        pub fn set_file(&mut self, file: Option<&Path>) -> Result<(), Taken> {
            let path = file.map(path_for);
            if path.as_ref() != self.0.as_ref().map(|listener| &listener.path) {
                self.0 = None;
                if let Some(path) = path {
                    self.0 = Listener::bind(path)?;
                }
            }
            Ok(())
        }

        /// Whether another window has `file` open — asked before this one takes it. That
        /// window is brought forward, as for a second launch; this one's own file is not
        /// asked about, since it would be asking itself and waiting on its own answer.
        pub fn open_elsewhere(&self, file: &Path) -> bool {
            let path = path_for(file);
            if self
                .0
                .as_ref()
                .is_some_and(|listener| listener.path == path)
            {
                return false;
            }
            hand_over(file, "", "", None, None)
        }

        pub fn try_recv(&self) -> Option<Goto> {
            self.0.as_ref()?.inbox.try_recv().ok()
        }
    }

    struct Listener {
        path: PathBuf,
        inbox: mpsc::Receiver<Goto>,
        stopped: Arc<AtomicBool>,
    }

    impl Listener {
        /// `Ok(None)` when the socket cannot be made for any reason but a live window
        /// holding it, which is `Err`.
        fn bind(path: PathBuf) -> Result<Option<Self>, Taken> {
            let listener = match UnixListener::bind(&path) {
                Ok(listener) => listener,
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    // Replace a socket left by a dead window. Keep a live socket or any
                    // other file that happens to share the name.
                    let Err(error) = UnixStream::connect(&path) else {
                        return Err(Taken);
                    };
                    let stale = error.kind() == io::ErrorKind::ConnectionRefused
                        && std::fs::symlink_metadata(&path)
                            .is_ok_and(|meta| meta.file_type().is_socket());
                    if !stale || std::fs::remove_file(&path).is_err() {
                        return Ok(None);
                    }
                    match UnixListener::bind(&path) {
                        Ok(listener) => listener,
                        Err(_) => return Ok(None),
                    }
                }
                Err(_) => return Ok(None),
            };
            let (requests, inbox) = mpsc::channel();
            let stopped = Arc::new(AtomicBool::new(false));
            let stop = Arc::clone(&stopped);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    if let Ok(stream) = stream
                        && serve(stream, &requests).is_err()
                    {
                        return;
                    }
                }
            });
            Ok(Some(Self {
                path,
                inbox,
                stopped,
            }))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            // Wake accept before unlinking. The next window can bind as soon as we return.
            let _ = UnixStream::connect(&self.path);
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// One launch's request: read it, pass it on, wait for the window's answer, say it.
    fn serve(stream: UnixStream, requests: &mpsc::Sender<Goto>) -> Result<(), ()> {
        let _ = stream.set_read_timeout(Some(PATIENCE));
        let mut line = Vec::new();
        let Ok(mut writer) = stream.try_clone() else {
            return Ok(());
        };
        if BufReader::new(stream).read_until(b'\n', &mut line).is_err() {
            return Ok(());
        }
        let line = line.strip_suffix(b"\n").unwrap_or(&line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let mut fields = line.split(|&byte| byte == b'\t');
        if fields.next() != Some(b"goto") {
            let _ = writeln!(writer, "no");
            return Ok(());
        }
        let mut field = || unescape(fields.next().unwrap_or_default());
        let (flow, node, declarations, after_save) = (field(), field(), field(), field());
        let (Ok(flow), Ok(node), Ok(after_save)) = (
            String::from_utf8(flow),
            String::from_utf8(node),
            String::from_utf8(after_save),
        ) else {
            let _ = writeln!(writer, "no");
            return Ok(());
        };
        let declarations =
            (!declarations.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(&declarations)));
        let (reply, answer) = mpsc::channel();
        requests
            .send(Goto {
                flow,
                node,
                declarations,
                after_save: (!after_save.trim().is_empty()).then_some(after_save),
                reply,
            })
            .map_err(|_| ())?;
        let took = answer.recv_timeout(PATIENCE).unwrap_or(false);
        let _ = writeln!(writer, "{}", if took { "ok" } else { "no" });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<OsString> {
        line.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn the_command_line_reads_a_file_a_flow_a_node_and_a_command() {
        let request = parse(args("talk.dln --flow road --node greeting")).unwrap();
        assert_eq!(request.file, Some(PathBuf::from("talk.dln")));
        assert_eq!(request.flow.as_deref(), Some("road"));
        assert_eq!(request.node.as_deref(), Some("greeting"));
        let request = parse(["--after-save".into(), "make dialogue".into()]).unwrap();
        assert_eq!(request.after_save.as_deref(), Some("make dialogue"));
        let request = parse(args("talk.dln --declarations names.json")).unwrap();
        assert_eq!(request.declarations, Some(PathBuf::from("names.json")));
        assert_eq!(parse(Vec::new()).unwrap(), Request::default());
    }

    #[test]
    fn a_wrong_command_line_says_what_is_wrong() {
        assert_eq!(parse(args("--flow")), Err("--flow needs a value".into()));
        assert_eq!(
            parse(args("--colour red")),
            Err("unknown option --colour".into())
        );
        assert_eq!(parse(args("a.dln b.dln")), Err("one file at a time".into()));
        assert_eq!(parse(args("--help")), Err(String::new()));
    }

    #[test]
    fn the_command_line_reads_exports_and_refuses_what_only_a_window_uses() {
        let request = parse(args(
            "talk.dln --export-ftl out/strings.ftl --export-recording - --export-screenplay s.md",
        ))
        .unwrap();
        assert_eq!(
            request.exports,
            vec![
                (Export::Ftl, "out/strings.ftl".into()),
                (Export::Recording, "-".into()),
                (Export::Screenplay, "s.md".into()),
            ],
            "in the order asked"
        );

        assert_eq!(
            parse(args("--export-ftl a.ftl")),
            Err("an export needs the FILE to export".into())
        );
        assert_eq!(
            parse(args("talk.dln --export-ftl a.ftl --export-ftl b.ftl")),
            Err("--export-ftl given twice".into())
        );
        assert_eq!(
            parse(args("talk.dln --export-ftl - --export-screenplay -")),
            Err("only one export can go to standard output (-)".into())
        );
        for window in ["--flow road", "--node intro", "--declarations n.json"] {
            let flag = window.split_whitespace().next().unwrap();
            assert_eq!(
                parse(args(&format!("talk.dln --export-ftl a.ftl {window}"))),
                Err(format!(
                    "{flag} is for the window, and an export opens none"
                ))
            );
        }
        assert_eq!(
            parse(args("talk.dln --export-ftl")),
            Err("--export-ftl needs a value".into())
        );
    }

    #[test]
    fn an_export_destination_cannot_be_a_flag() {
        for flag in ["--export-recording", "--unknown", "-h"] {
            assert_eq!(
                parse(args(&format!("talk.dln --export-ftl {flag}"))),
                Err("--export-ftl needs a value".into())
            );
        }
        let request = parse(args("talk.dln --export-ftl ./--export-recording")).unwrap();
        assert_eq!(
            request.exports,
            vec![(Export::Ftl, "./--export-recording".into())]
        );
    }

    #[test]
    fn an_export_refuses_to_replace_its_source() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("talk.dln");
        let original =
            r#"{"nodes":[{"id":1,"x":0,"y":0,"speaker":"Innkeeper","text":"Welcome"}],"edges":[]}"#;
        std::fs::write(&source, original).unwrap();

        #[cfg(unix)]
        let symlink = {
            let symlink = folder.path().join("source-link.ftl");
            std::os::unix::fs::symlink(&source, &symlink).unwrap();
            symlink
        };
        for destination in [
            source.clone(),
            folder.path().join("./talk.dln"),
            #[cfg(unix)]
            symlink,
        ] {
            let request = Request {
                file: Some(source.clone()),
                exports: vec![(Export::Ftl, destination)],
                ..Request::default()
            };
            let result = export(&request, &mut Vec::new());
            assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
            assert!(result.unwrap_err().contains("source document"));
        }
    }

    #[test]
    fn two_exports_to_one_file_are_refused_before_either_is_written() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("talk.dln");
        std::fs::write(
            &source,
            r#"{"nodes":[{"id":1,"x":0,"y":0,"speaker":"Innkeeper","text":"Welcome"}],"edges":[]}"#,
        )
        .unwrap();
        let out = folder.path().join("out.txt");

        #[cfg(unix)]
        let alias = {
            let alias = folder.path().join("alias.txt");
            std::os::unix::fs::symlink(&out, &alias).unwrap();
            alias
        };
        for second in [
            out.clone(),
            folder.path().join("./out.txt"),
            #[cfg(unix)]
            alias,
        ] {
            let request = Request {
                file: Some(source.clone()),
                exports: vec![(Export::Ftl, out.clone()), (Export::Screenplay, second)],
                ..Request::default()
            };
            let why = export(&request, &mut Vec::new()).unwrap_err();
            assert!(
                why.contains("--export-ftl and --export-screenplay both write it"),
                "{why}"
            );
            assert!(!out.exists(), "nothing written");
        }
    }

    #[test]
    fn an_export_writes_what_the_file_menu_writes_and_opens_nothing() {
        let folder = std::env::temp_dir().join(format!("palaver-export-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let mut doc = Doc::sample();
        let file = folder.join("talk.dln");
        doc.save(&file).unwrap();

        let ftl = folder.join("strings.ftl");
        let script = folder.join("talk.md");
        let line = format!(
            "{} --export-ftl {} --export-screenplay {} --export-recording -",
            file.display(),
            ftl.display(),
            script.display()
        );
        let mut out = Vec::new();
        let said = export(&parse(args(&line)).unwrap(), &mut out).unwrap();

        assert!(said.is_empty(), "the sample has no placeholders: {said:?}");
        assert_eq!(std::fs::read_to_string(&ftl).unwrap(), doc.to_ftl());
        assert_eq!(
            std::fs::read_to_string(&script).unwrap(),
            doc.to_screenplay()
        );
        assert_eq!(String::from_utf8(out).unwrap(), doc.to_recording_script());
        assert!(
            !folder.join("strings.ftl.tmp").exists(),
            "written beside and renamed"
        );

        // A placeholder is said on stderr, not refused: the build goes on, as "Export anyway".
        let id = doc.nodes[0].id;
        doc.write_placeholder(crate::doc::Line::Node(id), "Evening.", id);
        doc.save(&file).unwrap();
        let said = export(&parse(args(&line)).unwrap(), &mut Vec::new()).unwrap();
        assert_eq!(
            said,
            ["1 line is still a placeholder; it is marked in the export"]
        );

        let missing = folder.join("gone.dln");
        let request = parse(args(&format!("{} --export-ftl -", missing.display()))).unwrap();
        let failed = export(&request, &mut Vec::new()).unwrap_err();
        assert!(
            failed.starts_with(&missing.display().to_string()),
            "{failed}"
        );

        let request = parse(args(&format!(
            "{} --export-ftl {}",
            file.display(),
            folder.join("no/such/folder.ftl").display()
        )))
        .unwrap();
        assert!(
            export(&request, &mut Vec::new())
                .unwrap_err()
                .contains("no/such/folder.ftl")
        );

        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_after_save_command_runs_beside_the_file_and_says_why_it_failed() {
        let folder = std::env::temp_dir().join(format!("palaver-link-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("talk.dln");
        let done = run_after_save("test \"$PALAVER_FILE\" = \"$PWD/talk.dln\"", &file);
        assert!(done.ok, "{done:?}");
        let failed = run_after_save("echo fine; echo 'no flow named x' >&2; exit 3", &file);
        assert_eq!(
            failed,
            Outcome {
                ok: false,
                detail: "exit 3: no flow named x".into()
            }
        );
    }

    /// A path as a person types it at a prompt: relative, in a subfolder. The command runs
    /// in that subfolder, so the name it is handed has to still find the file from there.
    #[cfg(unix)]
    #[test]
    fn an_after_save_command_is_handed_a_path_that_finds_the_file() {
        let folder = std::env::temp_dir().join(format!("palaver-rel-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("sub/talk.dln"), "{}").unwrap();
        let relative = pathdiff(&folder.join("sub/talk.dln"));
        assert!(relative.is_relative());

        let done = run_after_save(
            "test -e \"$PALAVER_FILE\" && case \"$PALAVER_FILE\" in /*) ;; *) exit 1 ;; esac",
            &relative,
        );
        assert!(done.ok, "{done:?}");
    }

    /// `target` as a path relative to the working directory, without changing it: tests
    /// share one process, and its working directory with it.
    #[cfg(unix)]
    fn pathdiff(target: &Path) -> PathBuf {
        let here = std::env::current_dir().unwrap();
        let up: PathBuf = here.components().skip(1).map(|_| "..").collect();
        up.join(target.strip_prefix("/").unwrap())
    }

    #[cfg(unix)]
    fn wait_until<T>(mut check: impl FnMut() -> Option<T>) -> T {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(value) = check() {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for test fixture"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    #[test]
    fn after_save_hooks_publish_in_save_order() {
        use std::time::{Duration, Instant};

        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("talk.dln");
        let output = folder.path().join("strings.ftl");
        std::fs::write(&file, "old").unwrap();
        let mut queue = AfterSaveQueue::default();
        let mut first = queue
            .enqueue(
                r#"cp "$PALAVER_FILE" first && touch ready || exit
                   attempts=0
                   until test -e release; do
                       attempts=$((attempts+1))
                       test "$attempts" -lt 500 || exit 1
                       sleep 0.01
                   done
                   mv first strings.ftl"#
                    .into(),
                file.clone(),
            )
            .unwrap();
        wait_until(|| folder.path().join("ready").exists().then_some(()));
        std::fs::write(&file, "new").unwrap();
        let mut second = queue
            .enqueue(r#"cp "$PALAVER_FILE" strings.ftl"#.into(), file)
            .unwrap();

        // The first hook is still waiting to publish its snapshot. The second must queue.
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            assert_eq!(second.try_recv().unwrap(), None);
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(folder.path().join("release"), "").unwrap();
        assert!(wait_until(|| first.try_recv().unwrap()).ok);
        assert!(wait_until(|| second.try_recv().unwrap()).ok);
        assert_eq!(std::fs::read_to_string(output).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn a_second_launch_hands_its_place_to_the_window_with_the_file() {
        use std::os::unix::ffi::OsStrExt;

        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("talk.dln");
        let names = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/names-\xff\t\\n.json"));
        let command = "make \\\n\tdialogue\nprintf 'a\\tb'";
        std::fs::write(&file, "{}").unwrap();

        assert!(!hand_over(&file, "road", "greeting", None, None));
        let mut window = Handovers::default();
        window.set_file(Some(&file)).unwrap();
        std::thread::scope(|scope| {
            let launch =
                scope.spawn(|| hand_over(&file, "road", "greeting", Some(names), Some(command)));
            let request = wait_until(|| window.try_recv());
            assert_eq!(request.flow, "road");
            assert_eq!(request.node, "greeting");
            assert_eq!(request.declarations.as_deref(), Some(names));
            assert_eq!(
                request.after_save.as_deref(),
                Some(command),
                "byte for byte"
            );
            request.reply.send(true).unwrap();
            assert!(launch.join().unwrap());
        });
    }

    #[cfg(unix)]
    #[test]
    fn switching_documents_releases_the_old_listener_for_a_new_window() {
        let folder = tempfile::tempdir().unwrap();
        let first = folder.path().join("first.dln");
        let second = folder.path().join("second.dln");
        std::fs::write(&first, "{}").unwrap();
        std::fs::write(&second, "{}").unwrap();

        let mut original = Handovers::default();
        original.set_file(Some(&first)).unwrap();
        original.set_file(Some(&second)).unwrap();
        assert!(!hand_over(&first, "", "", None, None));
        let mut reopened = Handovers::default();
        reopened.set_file(Some(&first)).unwrap();

        std::thread::scope(|scope| {
            let launch_first = scope.spawn(|| hand_over(&first, "", "reopened", None, None));
            let launch_second = scope.spawn(|| hand_over(&second, "", "current", None, None));
            let new_request = wait_until(|| reopened.try_recv());
            assert_eq!(new_request.node, "reopened");
            new_request.reply.send(true).unwrap();
            let current_request = wait_until(|| original.try_recv());
            assert_eq!(current_request.node, "current");
            current_request.reply.send(true).unwrap();
            assert!(launch_first.join().unwrap());
            assert!(launch_second.join().unwrap());
        });

        original.set_file(None).unwrap();
        drop(reopened);
        assert!(!hand_over(&first, "", "", None, None));
        assert!(!hand_over(&second, "", "", None, None));
    }

    #[cfg(unix)]
    #[test]
    fn a_window_does_not_take_a_file_another_window_has_open() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("talk.dln");
        std::fs::write(&file, "{}").unwrap();

        let mut first = Handovers::default();
        first.set_file(Some(&file)).unwrap();
        assert!(
            !first.open_elsewhere(&file),
            "its own file is not elsewhere"
        );
        let mut second = Handovers::default();
        assert!(second.set_file(Some(&file)).is_err(), "the socket is taken");
        let second = second;
        let file_ref = &file;
        // Moved to the asking thread and back: a `Handovers` is one window's, not shared.
        let mut second = std::thread::scope(|scope| {
            let asked = scope.spawn(move || (second.open_elsewhere(file_ref), second));
            let request = wait_until(|| first.try_recv());
            assert_eq!((request.flow.as_str(), request.node.as_str()), ("", ""));
            assert_eq!(request.after_save, None);
            request.reply.send(true).unwrap();
            let (elsewhere, second) = asked.join().unwrap();
            assert!(elsewhere);
            second
        });
        drop(first);
        assert!(!second.open_elsewhere(&file));
        second.set_file(Some(&file)).unwrap();
    }
}

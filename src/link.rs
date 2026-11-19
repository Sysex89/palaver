//! The editor from outside: what the command line asks for, the one window a file is open
//! in, and a command run after each save.
//!
//! `palaver [FILE] [--flow NAME] [--node ALIAS|ID] [--after-save COMMAND]
//! [--declarations FILE]` opens FILE, shows the flow and selects the node, so another tool
//! (a game's level editor, a script) can open a conversation at the line it is about. `--node` takes what [`Doc::find_node`]
//! takes: an alias, or an id, never a position.
//!
//! **A file is edited in one window**, because two would each save over the other's work.
//! The first window to open a file from the command line listens on a socket named for
//! that file (`temp_dir()/palaver-<hash>.sock`, Unix only), and a later launch for the same
//! file hands its flow and node to that window and exits instead of opening a second one.
//! The window answers whether it still has that file — a person may have opened another
//! since — and if it does not, the launch opens the file itself. A socket left by a window
//! that was killed is found dead by the next launch and replaced.
//!
//! **`--after-save` is a shell command run after each successful save**, in the file's
//! folder, with `PALAVER_FILE` set to its path: the build step a file feeds (a converter, an
//! exporter) that would otherwise be forgotten. It runs off the foreground, and the status
//! bar says how it went; a save is never undone by a failing command.
//!
//! **`--declarations` names a JSON file of variables and characters** the conversation may
//! use without declaring them ([`Declarations`]): the tool's own names, a game's quests and
//! people, shown in the panes as declared from outside, locked, and never written into the
//! `.dln`. A later launch hands its file over with its place, so a window already open
//! picks up names the tool has added since.
//!
//! [`Doc::find_node`]: crate::doc::Doc::find_node
//! [`Declarations`]: crate::doc::Declarations
use std::path::{Path, PathBuf};
use std::process::Command;

pub const USAGE: &str = "usage: palaver [FILE] [--flow NAME] [--node ALIAS|ID] \
     [--after-save COMMAND] [--declarations FILE]";

/// What the command line asked for.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub file: Option<PathBuf>,
    pub flow: Option<String>,
    pub node: Option<String>,
    pub after_save: Option<String>,
    pub declarations: Option<PathBuf>,
}

/// Reads the arguments after the program's name. `Err("")` is a request for the usage
/// line; any other `Err` says what was wrong with them.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Request, String> {
    let mut request = Request::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--flow" => request.flow = Some(value("--flow")?),
            "--node" => request.node = Some(value("--node")?),
            "--after-save" => request.after_save = Some(value("--after-save")?),
            "--declarations" => {
                request.declarations = Some(PathBuf::from(value("--declarations")?));
            }
            "-h" | "--help" => return Err(String::new()),
            option if option.starts_with("--") => return Err(format!("unknown option {option}")),
            file => {
                if request.file.is_some() {
                    return Err("one file at a time".into());
                }
                request.file = Some(PathBuf::from(file));
            }
        }
    }
    Ok(request)
}

/// How an after-save command went: whether it succeeded, and a line to say about it.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub detail: String,
}

/// Runs `command` through the shell in `file`'s folder, with `PALAVER_FILE` set. Blocking:
/// the caller runs it off the foreground. The line said about a failure is the last one
/// the command wrote to stderr, else to stdout, since that is where a tool says why.
pub fn run_after_save(command: &str, file: &Path) -> Outcome {
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
pub use socket::{hand_over, listen};

#[cfg(unix)]
mod socket {
    use std::hash::{Hash, Hasher};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Duration;

    /// How long either side waits for the other before giving up.
    const PATIENCE: Duration = Duration::from_secs(3);

    /// A request handed over from a later launch: where to go, and where to answer
    /// whether this window took it.
    pub struct Goto {
        pub flow: String,
        pub node: String,
        /// A declarations file to read again, or empty.
        pub declarations: String,
        pub reply: mpsc::Sender<bool>,
    }

    /// The socket for a file: one per file, so two files open in two windows each have
    /// their own. Named by a hash of the canonical path, which is short, the same for
    /// every way of spelling the path, and safe as a file name.
    pub fn path_for(file: &Path) -> PathBuf {
        let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut hasher);
        std::env::temp_dir().join(format!("palaver-{:016x}.sock", hasher.finish()))
    }

    /// A field of the one-line protocol: tabs and newlines are its separators.
    fn clean(field: &str) -> String {
        field.replace(['\t', '\n', '\r'], " ")
    }

    /// Hands `flow`, `node` and a declarations file (empty for none) to the window that
    /// has `file` open. True when that window took them; false when there is none, or it
    /// has another file open now. The declarations ride as a fourth field, which a window
    /// from before them reads past.
    pub fn hand_over(file: &Path, flow: &str, node: &str, declarations: &str) -> bool {
        let Ok(mut stream) = UnixStream::connect(path_for(file)) else {
            return false;
        };
        let _ = stream.set_read_timeout(Some(PATIENCE));
        let line = format!(
            "goto\t{}\t{}\t{}",
            clean(flow),
            clean(node),
            clean(declarations)
        );
        if writeln!(stream, "{line}").is_err() {
            return false;
        }
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim() == "ok"
    }

    /// Listens for later launches for `file`, each delivered as a [`Goto`] the window
    /// answers. `None` when the socket cannot be made; the window then simply has no
    /// handover, which costs a second window at worst.
    pub fn listen(file: &Path) -> Option<mpsc::Receiver<Goto>> {
        let path = path_for(file);
        // A socket nothing answers on was left by a window that died: replace it. One that
        // answers belongs to a live window, and is left alone.
        if UnixStream::connect(&path).is_err() {
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path).ok()?;
        let (requests, inbox) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if serve(stream, &requests).is_err() {
                    return; // The window is gone.
                }
            }
        });
        Some(inbox)
    }

    /// One launch's request: read it, pass it on, wait for the window's answer, say it.
    fn serve(stream: UnixStream, requests: &mpsc::Sender<Goto>) -> Result<(), ()> {
        let _ = stream.set_read_timeout(Some(PATIENCE));
        let mut line = String::new();
        let Ok(mut writer) = stream.try_clone() else {
            return Ok(());
        };
        if BufReader::new(stream).read_line(&mut line).is_err() {
            return Ok(());
        }
        let mut fields = line.trim_end_matches(['\n', '\r']).split('\t');
        if fields.next() != Some("goto") {
            let _ = writeln!(writer, "no");
            return Ok(());
        }
        let flow = fields.next().unwrap_or_default().to_string();
        let node = fields.next().unwrap_or_default().to_string();
        let declarations = fields.next().unwrap_or_default().to_string();
        let (reply, answer) = mpsc::channel();
        requests
            .send(Goto {
                flow,
                node,
                declarations,
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

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_command_line_reads_a_file_a_flow_a_node_and_a_command() {
        let request = parse(args("talk.dln --flow road --node greeting")).unwrap();
        assert_eq!(request.file, Some(PathBuf::from("talk.dln")));
        assert_eq!(request.flow.as_deref(), Some("road"));
        assert_eq!(request.node.as_deref(), Some("greeting"));
        let request = parse(["--after-save".to_string(), "make dialogue".to_string()]).unwrap();
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
    #[test]
    fn a_second_launch_hands_its_place_to_the_window_with_the_file() {
        let folder = std::env::temp_dir().join(format!("palaver-socket-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("talk.dln");
        std::fs::write(&file, "{}").unwrap();

        assert!(
            !hand_over(&file, "road", "greeting", ""),
            "nothing listens yet"
        );
        let inbox = listen(&file).expect("a socket");
        let window = std::thread::spawn(move || {
            let first = inbox.recv().unwrap();
            first.reply.send(true).unwrap();
            let second = inbox.recv().unwrap();
            second.reply.send(false).unwrap(); // another file is open by now
            (first.flow, first.node, first.declarations)
        });
        assert!(hand_over(&file, "road", "greeting", "/tmp/names.json"));
        assert!(!hand_over(&file, "road", "greeting", ""));
        assert_eq!(
            window.join().unwrap(),
            (
                "road".to_string(),
                "greeting".to_string(),
                "/tmp/names.json".to_string()
            )
        );
    }
}

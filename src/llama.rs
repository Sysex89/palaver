//! The placeholder generator's engine: llama.cpp, in process, through `llama-cpp-2`.
//! **Compiled only with `--features generate`**, because it builds llama.cpp from source and
//! that wants cmake and a C++ toolchain a plain `cargo run` should not need.
//!
//! The model lives on a thread of its own and is loaded there on the first request, never at
//! startup and never on opting in: one job channel in, one event channel out, polled by the
//! view. It is handed nothing but the prompt text [`fill::Prompt::text`] builds, and hands back
//! nothing but a line [`fill::clean`] kept, so it never sees the document.
//!
//! Imports no `gpui`; [`Filler`] is the whole of what the view talks to.

use crate::doc::Line;
use crate::fill::{self, Prompt};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

/// Room for a long path through a conversation plus the line. A prompt that does not fit is
/// reported rather than cut, since a cut conversation is a different conversation.
const N_CTX: u32 = 4096;
const N_BATCH: usize = 512;

/// The loaded model. `LlamaBackend::init` may run once per process, so there is one of these
/// for the life of the editor.
struct Engine {
    backend: LlamaBackend,
    model: LlamaModel,
}

impl Engine {
    fn load(path: &Path) -> Result<Engine, String> {
        let mut backend = LlamaBackend::init().map_err(|e| e.to_string())?;
        // llama.cpp narrates every tensor it loads on stderr.
        backend.void_logs();
        let model = LlamaModel::load_from_file(&backend, path, &LlamaModelParams::default())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Engine { backend, model })
    }

    /// One line for `prompt`: the first of [`fill::ATTEMPTS`] seeds, counting up from the
    /// prompt's, whose answer survives [`fill::clean`]. Returns the line and the seed that
    /// produced it, which is what the mark stores — so regenerating reproduces it.
    fn line(&self, prompt: &Prompt) -> Result<(String, u64), String> {
        let text = prompt.text();
        for attempt in 0..fill::ATTEMPTS {
            let seed = prompt.seed.wrapping_add(attempt);
            let raw = self.complete(&text, seed)?;
            if let Some(line) = fill::clean(&raw, &prompt.who) {
                return Ok((line, seed));
            }
        }
        Err("the model gave no usable line".into())
    }

    /// Samples from `text` until a newline after something has been said, an end-of-turn
    /// token, or [`fill::MAX_TOKENS`]. A fresh context per line: the prompts are short and
    /// the model small, so prefill is the cheap half, and a fresh context is what makes the
    /// same seed give the same line no matter what was generated before it.
    ///
    /// ponytail: no prefix reuse between siblings yet. Batch fill is where it pays; the
    /// recurrent layers cannot be rewound, so it has to be a `seq_cp` fork, not a rewind.
    fn complete(&self, text: &str, seed: u64) -> Result<String, String> {
        // A fixed thread count, so the same seed gives the same line on the same machine.
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_batch(N_BATCH as u32)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = self
            .model
            .new_context(&self.backend, params)
            .map_err(|e| e.to_string())?;
        let vocab = self.model.vocab();
        // `parse_special`, so the ChatML markers become their control tokens.
        let tokens = vocab.tokenize(text.as_bytes(), vocab.should_add_bos(), true);
        if tokens.len() + fill::MAX_TOKENS >= N_CTX as usize {
            return Err("the conversation leading here is too long for the model".into());
        }

        let mut batch = LlamaBatch::new(N_BATCH, 1);
        for (chunk_at, chunk) in tokens.chunks(N_BATCH).enumerate() {
            batch.clear();
            for (i, &token) in chunk.iter().enumerate() {
                let pos = chunk_at * N_BATCH + i;
                let last = pos == tokens.len() - 1;
                batch
                    .add(token, pos as i32, &[0], last)
                    .map_err(|e| e.to_string())?;
            }
            ctx.decode(&mut batch).map_err(|e| e.to_string())?;
        }

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.9, 1),
            LlamaSampler::temp(fill::TEMPERATURE),
            LlamaSampler::dist((seed ^ (seed >> 32)) as u32),
        ]);
        let mut out: Vec<u8> = Vec::new();
        for pos in (tokens.len()..).take(fill::MAX_TOKENS) {
            // Samples and accepts, so the chain's state moves on by itself.
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if vocab.is_eog(token) {
                break;
            }
            out.extend(vocab.token_to_piece(token, false, None));
            let said = String::from_utf8_lossy(&out);
            if said.trim_start().contains('\n') {
                break;
            }
            batch.clear();
            batch
                .add(token, pos as i32, &[0], true)
                .map_err(|e| e.to_string())?;
            ctx.decode(&mut batch).map_err(|e| e.to_string())?;
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

struct Job {
    ticket: u32,
    prompt: Prompt,
}

enum Event {
    /// The first job arrived and the model is being read in.
    Loading,
    Done {
        ticket: u32,
        result: Result<(String, u64), String>,
    },
}

/// The thread that owns the model, and its two channels.
struct Worker {
    jobs: Sender<Job>,
    events: Receiver<Event>,
}

impl Worker {
    fn spawn(path: PathBuf) -> Result<Worker, String> {
        let (jobs, job_rx) = channel::<Job>();
        let (event_tx, events) = channel::<Event>();
        std::thread::Builder::new()
            .name("palaver-generate".into())
            .spawn(move || {
                let mut engine: Option<Result<Engine, String>> = None;
                // Ends when the editor drops its sender.
                while let Ok(job) = job_rx.recv() {
                    let engine = engine.get_or_insert_with(|| {
                        event_tx.send(Event::Loading).ok();
                        Engine::load(&path)
                    });
                    let result = match engine {
                        Ok(engine) => engine.line(&job.prompt),
                        Err(error) => Err(format!("could not load the model: {error}")),
                    };
                    let done = Event::Done {
                        ticket: job.ticket,
                        result,
                    };
                    if event_tx.send(done).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Worker { jobs, events })
    }
}

/// A line being written: which one, and what it said when it was asked for, so a result is
/// only written over the text it was generated for.
struct Writing {
    ticket: u32,
    line: Line,
    was: String,
}

/// A generation that has come back.
pub struct Finished {
    pub line: Line,
    /// What the line said when it was asked for. The view writes the result only if it
    /// still says that: a human line is never overwritten.
    pub was: String,
    pub result: Result<(String, u64), String>,
}

/// One line at a time, on the model thread. The view's whole interface to the engine.
#[derive(Default)]
pub struct Filler {
    worker: Option<Worker>,
    writing: Option<Writing>,
    /// Bumped per job and by [`Filler::forget`], so a result for a document that has left
    /// is recognised and dropped.
    ticket: u32,
    loading: bool,
}

impl Filler {
    /// What the status bar says while a job runs.
    pub fn busy(&self) -> Option<&'static str> {
        match (&self.writing, self.loading) {
            (None, _) => None,
            (Some(_), true) => Some("Loading model…"),
            (Some(_), false) => Some("Writing a line…"),
        }
    }

    /// Sends `prompt` to the model thread, starting it — and finding the model — on first
    /// use. Refused while another line is being written: one at a time keeps "which line
    /// did this come back for" a single field.
    pub fn start(&mut self, line: Line, prompt: Prompt, was: String) -> Result<(), String> {
        if self.writing.is_some() {
            return Err("Still writing the last line.".into());
        }
        if self.worker.is_none() {
            let Some(path) = fill::find_model() else {
                let dir = fill::model_dirs()
                    .first()
                    .map_or("models/".into(), |d| d.display().to_string());
                return Err(format!("No model found. Put a .gguf file in {dir}"));
            };
            self.worker = Some(Worker::spawn(path)?);
        }
        self.ticket = self.ticket.wrapping_add(1);
        let job = Job {
            ticket: self.ticket,
            prompt,
        };
        let worker = self.worker.as_ref().expect("spawned above");
        if worker.jobs.send(job).is_err() {
            self.worker = None;
            return Err("The model thread stopped.".into());
        }
        self.writing = Some(Writing {
            ticket: self.ticket,
            line,
            was,
        });
        Ok(())
    }

    /// The line being written, if one is.
    pub fn writing(&self) -> Option<Line> {
        self.writing.as_ref().map(|w| w.line)
    }

    /// Whether a job is out, so the view knows to keep polling.
    pub fn is_writing(&self) -> bool {
        self.writing.is_some()
    }

    /// The finished job, if one came back since the last call. Never blocks.
    pub fn poll(&mut self) -> Option<Finished> {
        loop {
            let event = match self.worker.as_ref()?.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.loading = false;
                    let writing = self.writing.take()?;
                    return Some(Finished {
                        line: writing.line,
                        was: writing.was,
                        result: Err("the model thread stopped".into()),
                    });
                }
            };
            match event {
                Event::Loading => self.loading = true,
                Event::Done { ticket, result } => {
                    self.loading = false;
                    // A job `forget` disowned: its line belongs to a document that left.
                    let Some(writing) = self.writing.take_if(|w| w.ticket == ticket) else {
                        continue;
                    };
                    return Some(Finished {
                        line: writing.line,
                        was: writing.was,
                        result,
                    });
                }
            }
        }
    }

    /// The document was swapped: whatever is being written now has nowhere to land. The
    /// thread still finishes it, and [`Filler::poll`] drops the result.
    pub fn forget(&mut self) {
        self.writing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Doc, Line};

    /// Against the real model, so skipped when none is installed. Same seed, same line; and
    /// whatever comes back is one line.
    #[test]
    fn the_same_seed_writes_the_same_single_line() {
        let Some(path) = fill::find_model() else {
            eprintln!("no model found; skipping");
            return;
        };
        let engine = Engine::load(&path).unwrap();
        let mut doc = Doc::sample();
        doc.premise = "A tavern on a snowed-in mountain pass. Low fantasy, dry humour.".into();
        let id = doc
            .nodes
            .iter()
            .find(|n| doc.edges.iter().any(|e| e.to == n.id) && !n.speaker.is_empty())
            .unwrap()
            .id;
        let prompt = fill::prompt(&doc, Line::Node(id)).unwrap();
        let (first, seed) = engine.line(&prompt).unwrap();
        let (again, again_seed) = engine.line(&prompt).unwrap();
        eprintln!("{}\n→ {first}", prompt.text());
        assert_eq!((first.as_str(), seed), (again.as_str(), again_seed));
        assert!(!first.contains('\n'));
    }

    /// Not an assertion but a report, for judging the prompt by eye: every written line and
    /// choice in the sample, blanked and regenerated, beside what the author wrote.
    /// `cargo test --features generate quality_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn quality_report() {
        let Some(path) = fill::find_model() else {
            return;
        };
        let engine = Engine::load(&path).unwrap();
        let mut doc = Doc::sample();
        doc.premise = "A tavern on a snowed-in mountain pass. Low fantasy, dry humour.".into();
        let mut lines: Vec<Line> = doc
            .nodes
            .iter()
            .filter(|n| fill::prompt(&doc, Line::Node(n.id)).is_some() && !n.text.is_empty())
            .map(|n| Line::Node(n.id))
            .collect();
        lines.extend(
            doc.edges
                .iter()
                .filter(|e| !e.label.is_empty())
                .map(|e| Line::Edge(e.from, e.to))
                .filter(|&l| fill::prompt(&doc, l).is_some()),
        );
        let verbose = std::env::var_os("SHOW_PROMPTS").is_some();
        for line in lines {
            let mut blanked = Doc::from_json(&doc.to_json()).unwrap();
            let original = blanked.line_text(line).unwrap().to_string();
            blanked.write_placeholder(line, "", 0);
            blanked.accept_line(line);
            let prompt = fill::prompt(&blanked, line).unwrap();
            // For comparing models outside this engine: one JSON object per line.
            if let Some(dump) = std::env::var_os("DUMP_PROMPTS") {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dump)
                    .unwrap();
                let row = serde_json::json!({
                    "who": prompt.who, "was": original, "prompt": prompt.text(), "seed": prompt.seed,
                });
                writeln!(file, "{row}").unwrap();
            }
            if verbose {
                eprintln!("{}", prompt.text());
            }
            let (got, _) = engine.line(&prompt).unwrap();
            eprintln!(
                "{:>9}  was: {original}\n           got: {got}\n",
                prompt.who
            );
        }
    }
}

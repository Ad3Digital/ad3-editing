//! Native preview worker. JSON requests on stdin; length-prefixed JSON + RGBA on stdout.
//! FFmpeg owns codec state outside Chromium. Each clip has one bounded request slot.
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
};

#[derive(Deserialize, Debug)]
struct Request {
    id: u64,
    session: String,
    op: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    time: f64,
    #[serde(default)]
    width: usize,
    #[serde(default)]
    height: usize,
}
#[derive(Serialize)]
struct Reply<'a> {
    id: u64,
    time: f64,
    width: usize,
    height: usize,
    bytes: usize,
    error: Option<&'a str>,
}
type Output = Arc<Mutex<io::Stdout>>;
type Process = Arc<Mutex<Option<Child>>>;

fn reply(out: &Output, req: &Request, time: f64, pixels: &[u8], error: Option<&str>) {
    let header = serde_json::to_vec(&Reply {
        id: req.id,
        time,
        width: req.width,
        height: req.height,
        bytes: pixels.len(),
        error,
    })
    .unwrap();
    let mut out = out.lock().unwrap();
    let _ = out
        .write_all(&(header.len() as u32).to_le_bytes())
        .and_then(|_| out.write_all(&header))
        .and_then(|_| out.write_all(pixels))
        .and_then(|_| out.flush());
}
fn stop(process: &Process) {
    if let Some(mut child) = process.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}
fn valid(req: &Request) -> bool {
    req.time.is_finite()
        && req.time >= 0.0
        && req.width > 0
        && req.height > 0
        && req.width <= 1280
        && req.height <= 1280
        && req.width * req.height <= 921600
        && Path::new(&req.source).is_absolute()
        && Path::new(&req.source).is_file()
}
fn needs_restart(started: bool, current: f64, requested: f64) -> bool {
    !started || requested < current - 0.017 || requested > current + 2.0
}
struct Stream {
    process: Process,
    reader: Option<BufReader<ChildStdout>>,
    pixels: Vec<u8>,
    source: String,
    width: usize,
    height: usize,
    time: f64,
    start: f64,
    index: u64,
}
impl Stream {
    fn frame(&mut self, req: &Request, ffmpeg: &str) -> Result<f64, String> {
        if self.source != req.source
            || self.width != req.width
            || self.height != req.height
            || needs_restart(self.reader.is_some(), self.time, req.time)
        {
            stop(&self.process);
            self.reader = None;
            let mut cmd = Command::new(ffmpeg);
            cmd.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-threads",
                "4",
                "-protocol_whitelist",
                "file,pipe",
                "-ss",
                &format!("{:.6}", req.time),
                "-i",
                &req.source,
                "-an",
                "-sn",
                "-dn",
                "-map",
                "0:v:0",
                "-filter_threads",
                "1",
                "-vf",
                &format!(
                    "fps=30,scale={}:{}:flags=fast_bilinear",
                    req.width, req.height
                ),
                "-threads",
                "1",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let mut child = cmd
                .spawn()
                .map_err(|e| format!("Cannot start FFmpeg: {e}"))?;
            self.reader = Some(BufReader::new(child.stdout.take().unwrap()));
            *self.process.lock().unwrap() = Some(child);
            self.source = req.source.clone();
            self.width = req.width;
            self.height = req.height;
            self.pixels.resize(req.width * req.height * 4, 0);
            self.start = req.time;
            self.index = 0;
            self.time = -1.0;
        }
        while self.time < req.time - 0.017 {
            if let Err(e) = self.reader.as_mut().unwrap().read_exact(&mut self.pixels) {
                self.reader = None;
                stop(&self.process);
                return Err(format!("Native video frame unavailable: {e}"));
            }
            self.time = self.start + self.index as f64 / 30.0;
            self.index += 1;
        }
        Ok(self.time)
    }
}
struct Session {
    tx: mpsc::SyncSender<Request>,
    process: Process,
    thread: thread::JoinHandle<()>,
}
fn main() {
    let ffmpeg = std::env::args().nth(1).unwrap_or_else(|| "ffmpeg".into());
    let output = Arc::new(Mutex::new(io::stdout()));
    let mut sessions: HashMap<String, Session> = HashMap::new();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if req.op == "close" {
            if let Some(session) = sessions.remove(&req.session) {
                stop(&session.process);
                drop(session.tx);
                let _ = session.thread.join();
            }
            continue;
        }
        if req.op != "frame" || !valid(&req) {
            reply(
                &output,
                &req,
                0.0,
                &[],
                Some("Invalid native preview request"),
            );
            continue;
        }
        if !sessions.contains_key(&req.session) {
            if sessions.len() >= 12 {
                reply(
                    &output,
                    &req,
                    0.0,
                    &[],
                    Some("Too many native preview sessions"),
                );
                continue;
            }
            let (tx, rx) = mpsc::sync_channel::<Request>(1);
            let process = Arc::new(Mutex::new(None));
            let proc = process.clone();
            let out = output.clone();
            let bin = ffmpeg.clone();
            let handle = thread::spawn(move || {
                let mut stream = Stream {
                    process: proc,
                    reader: None,
                    pixels: vec![],
                    source: String::new(),
                    width: 0,
                    height: 0,
                    time: -1.0,
                    start: 0.0,
                    index: 0,
                };
                while let Ok(req) = rx.recv() {
                    match stream.frame(&req, &bin) {
                        Ok(time) => reply(&out, &req, time, &stream.pixels, None),
                        Err(e) => reply(&out, &req, 0.0, &[], Some(&e)),
                    }
                }
                stop(&stream.process);
            });
            sessions.insert(
                req.session.clone(),
                Session {
                    tx,
                    process,
                    thread: handle,
                },
            );
        }
        if let Err(e) = sessions[&req.session].tx.try_send(req) {
            let req = match e {
                mpsc::TrySendError::Full(r) | mpsc::TrySendError::Disconnected(r) => r,
            };
            reply(&output, &req, 0.0, &[], Some("Native preview busy"));
        }
    }
    for (_, session) in sessions {
        stop(&session.process);
        drop(session.tx);
        let _ = session.thread.join();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shuttle_continues_without_restarting_codec() {
        assert!(!needs_restart(true, 20.0, 20.067));
        assert!(!needs_restart(true, 20.0, 20.167));
        assert!(needs_restart(true, 20.0, 18.0));
        assert!(needs_restart(true, 20.0, 25.0));
    }
    #[test]
    fn rejects_network_and_unbounded_frames() {
        let r = Request {
            id: 1,
            session: "a".into(),
            op: "frame".into(),
            source: "https://host/video".into(),
            time: 0.0,
            width: 960,
            height: 540,
        };
        assert!(!valid(&r));
    }
}

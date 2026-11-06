//! Two independent mixes built from PulseAudio-compatible virtual devices.
//! A pipe-owned helper holds the graph so parent crashes also trigger cleanup.
use crate::audio::{self, Item, Kind, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub const PREFIX: &str = "oto_oto_mix_";
pub const SOURCE: &str = "oto_oto_mix_stream";
pub const CHANNELS: [&str; 5] = ["Music", "Game", "Voice", "Browser", "Microphone"];
const KEYS: [&str; 5] = ["music", "game", "voice", "browser", "microphone"];
const APP_ID: &str = "io.oto-oto.mixer";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Bus {
    Listen,
    Stream,
}
impl Bus {
    pub fn index(self) -> usize {
        if self == Self::Listen { 0 } else { 1 }
    }
    pub fn name(self) -> &'static str {
        if self == Self::Listen {
            "Listen"
        } else {
            "Stream"
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Level {
    pub volume: u32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixState {
    pub active: bool,
    pub monitor: String,
    pub microphone: Option<String>,
    pub levels: [[Level; 2]; 5],
    pub masters: [Level; 2],
    pub sinks: [Option<u32>; 5],
}
impl Default for MixState {
    fn default() -> Self {
        let level = Level {
            volume: 70,
            muted: false,
        };
        let mut levels = [[level; 2]; 5];
        levels[4][0].muted = true;
        Self {
            active: false,
            monitor: String::new(),
            microphone: None,
            levels,
            masters: [Level {
                volume: 100,
                muted: false,
            }; 2],
            sinks: [None; 5],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MixAction {
    Start {
        monitor: String,
        microphone: Option<String>,
    },
    Stop,
    Level {
        channel: Option<usize>,
        bus: Bus,
        level: Level,
    },
    Assign {
        stream: u32,
        channel: Option<usize>,
    },
    Inspect,
}

impl MixState {
    pub fn preview(&mut self, action: &MixAction) {
        match action {
            MixAction::Level {
                channel,
                bus,
                level,
            } => {
                let mut level = *level;
                level.volume = level.volume.min(100);
                if let Some(i) = channel {
                    if let Some(pair) = self.levels.get_mut(*i) {
                        pair[bus.index()] = level;
                    }
                } else {
                    self.masters[bus.index()] = level;
                }
            }
            MixAction::Start {
                monitor,
                microphone,
            } => {
                self.monitor = monitor.clone();
                self.microphone = microphone.clone();
            }
            _ => {}
        }
    }
    pub fn level(&self, channel: Option<usize>, bus: Bus) -> Level {
        channel
            .map(|i| self.levels[i][bus.index()])
            .unwrap_or(self.masters[bus.index()])
    }
}

#[derive(Serialize, Deserialize)]
struct Reply {
    state: MixState,
    error: Option<String>,
}

pub struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    pub state: MixState,
}
impl Client {
    pub fn running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }
    pub fn spawn() -> Result<Self, String> {
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command
            .arg("--mix-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // A terminal interrupt should close the UI's pipe, leaving this process alive long
        // enough to restore routes. It must not receive the UI process group's signal.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start stream mixer: {e}"))?;
        Ok(Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            state: MixState::default(),
        })
    }
    pub fn request(&mut self, action: &MixAction) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("Stream mixer has closed")?;
        serde_json::to_writer(&mut *input, action).map_err(|e| e.to_string())?;
        input
            .write_all(b"\n")
            .and_then(|_| input.flush())
            .map_err(|e| format!("Stream mixer disconnected: {e}"))?;
        let mut line = String::new();
        if self
            .output
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            self.state.active = false;
            self.state.sinks = [None; 5];
            return Err("Stream mixer exited unexpectedly. Start it again to reconnect.".into());
        }
        let reply: Reply = serde_json::from_str(&line)
            .map_err(|e| format!("Invalid stream mixer response: {e}"))?;
        self.state = reply.state;
        reply.error.map_or(Ok(()), Err)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // EOF lets the helper restore streams and unload its modules, including after a UI crash.
        self.input.take();
        let _ = self.child.wait();
    }
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
fn arg(key: &str, value: &str) -> String {
    format!("{key}={}", quoted(value))
}
fn run(args: &[&str]) -> Result<String, String> {
    audio::pactl(&args.iter().map(|s| (*s).into()).collect::<Vec<_>>())
}

pub fn number(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

struct Link {
    module: u32,
    channel: usize,
    bus: Bus,
    input: Option<u32>,
}
struct Session {
    _lock: File,
    token: String,
    modules: Vec<u32>,
    links: Vec<Link>,
    connection_modules: Vec<u32>,
    routes: BTreeMap<u32, String>,
    original_output: String,
    original_input: String,
    monitor: String,
    microphone: Option<String>,
}

impl Session {
    fn start(
        state: &mut MixState,
        monitor: &str,
        microphone: Option<&str>,
    ) -> Result<Self, String> {
        let before = audio::snapshot()?;
        if !before
            .outputs
            .iter()
            .any(|i| i.name == monitor && !i.internal)
        {
            return Err("Choose an available listening device before starting the mixer.".into());
        }
        if let Some(mic) = microphone
            && !before.inputs.iter().any(|i| i.name == mic && !i.internal)
        {
            return Err("The selected microphone is no longer available.".into());
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|p| std::path::PathBuf::from(p).join(".local/state/oto-oto"))
            })
            .ok_or("Cannot find a user runtime directory for the mixer")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("oto-oto-mixer.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|_| "Another Oto Oto stream mixer is already running.".to_string())?;
        if before.outputs.iter().any(|i| i.name.starts_with(PREFIX)) {
            return Err("Existing Oto Oto mix devices were found. Stop the other mixer before starting a new session.".into());
        }
        let mut session = Self {
            _lock: lock,
            token: format!(
                "oto-session-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            modules: vec![],
            links: vec![],
            connection_modules: vec![],
            routes: BTreeMap::new(),
            original_output: before.default_output,
            original_input: before.default_input,
            monitor: monitor.into(),
            microphone: microphone.map(str::to_owned),
        };
        // Keep both buses silent until every connection and its independent gain are ready.
        for (key, title) in [("listen_bus", "Listen Mix"), ("stream_bus", "Stream Mix")] {
            session.sink(key, title)?;
            run(&["set-sink-mute", &format!("{PREFIX}{key}"), "1"])?;
        }
        for (i, key) in KEYS.iter().enumerate() {
            session.sink(key, CHANNELS[i])?;
        }
        for (i, key) in KEYS.iter().enumerate() {
            for bus in [Bus::Listen, Bus::Stream] {
                let target = format!("{PREFIX}{}_bus", bus.name().to_lowercase());
                let module = session.loopback(&format!("{PREFIX}{key}.monitor"), &target)?;
                session.links.push(Link {
                    module,
                    channel: i,
                    bus,
                    input: None,
                });
            }
        }
        let inputs = audio::snapshot()?.playback;
        for link in &mut session.links {
            let input = inputs
                .iter()
                .find(|i| i.owner_module == Some(link.module))
                .ok_or_else(|| {
                    format!(
                        "The audio server did not create the {} connection.",
                        CHANNELS[link.channel]
                    )
                })?;
            link.input = Some(input.index);
            set_level(
                Kind::Playback,
                input.index.to_string(),
                state.levels[link.channel][link.bus.index()],
            )?;
        }
        session.loopback(&format!("{PREFIX}listen_bus.monitor"), monitor)?;
        let props = session.properties("Stream");
        session.load(vec![
            "module-remap-source".into(),
            arg("master", &format!("{PREFIX}stream_bus.monitor")),
            arg("source_name", SOURCE),
            arg("source_properties", &props),
            "rate=48000".into(),
            "channels=2".into(),
            "remix=yes".into(),
        ])?;
        run(&["set-source-volume", SOURCE, "100%"])?;
        run(&["set-source-mute", SOURCE, "0"])?;
        if let Some(mic) = microphone {
            session.loopback(mic, &format!("{PREFIX}microphone"))?;
        }
        for bus in [Bus::Listen, Bus::Stream] {
            set_level(
                Kind::Output,
                format!("{PREFIX}{}_bus", bus.name().to_lowercase()),
                state.masters[bus.index()],
            )?;
        }
        let after = audio::snapshot()?;
        for (i, key) in KEYS.iter().enumerate() {
            state.sinks[i] = Some(
                after
                    .outputs
                    .iter()
                    .find(|s| s.name == format!("{PREFIX}{key}"))
                    .ok_or("A stream mixer channel disappeared during setup")?
                    .index,
            );
        }
        session.restore_defaults(&after)?;
        state.active = true;
        state.monitor = monitor.into();
        state.microphone = microphone.map(str::to_owned);
        Ok(session)
    }

    fn properties(&self, title: &str) -> String {
        format!(
            "device.description={} application.name={} application.id={APP_ID} oto.session={} module-stream-restore.id={}-{} node.dont-fallback=true node.virtual=true priority.session=0",
            quoted(&format!("Oto Oto {title}")),
            quoted("Oto Oto Mixer"),
            self.token,
            self.token,
            self.modules.len()
        )
    }
    fn load(&mut self, args: Vec<String>) -> Result<u32, String> {
        let mut command = vec!["load-module".into()];
        command.extend(args);
        let id = audio::pactl(&command)?
            .trim()
            .parse()
            .map_err(|_| "Invalid module id from audio server")?;
        self.modules.push(id);
        Ok(id)
    }
    fn sink(&mut self, key: &str, title: &str) -> Result<(), String> {
        self.load(vec![
            "module-null-sink".into(),
            arg("sink_name", &format!("{PREFIX}{key}")),
            arg("sink_properties", &self.properties(title)),
            "rate=48000".into(),
            "channels=2".into(),
            "channel_map=front-left,front-right".into(),
        ])?;
        // Session-restore policies can remember volume for virtual device names.
        run(&["set-sink-volume", &format!("{PREFIX}{key}"), "100%"])?;
        run(&["set-sink-mute", &format!("{PREFIX}{key}"), "0"])?;
        Ok(())
    }
    fn loopback(&mut self, source: &str, sink: &str) -> Result<u32, String> {
        let props = self.properties("Mix connection");
        let module = self.load(vec![
            "module-loopback".into(),
            arg("source", source),
            arg("sink", sink),
            "rate=48000".into(),
            "channels=2".into(),
            "latency_msec=20".into(),
            "source_dont_move=true".into(),
            "sink_dont_move=true".into(),
            arg("sink_input_properties", &props),
            arg("source_output_properties", &props),
        ])?;
        self.connection_modules.push(module);
        Ok(module)
    }
    fn level(&self, channel: Option<usize>, bus: Bus, level: Level) -> Result<(), String> {
        if let Some(channel) = channel {
            let link = self
                .links
                .iter()
                .find(|l| l.channel == channel && l.bus == bus)
                .ok_or("Unknown mix channel")?;
            set_level(
                Kind::Playback,
                link.input
                    .ok_or("Mix connection is unavailable")?
                    .to_string(),
                level,
            )
        } else {
            set_level(
                Kind::Output,
                format!("{PREFIX}{}_bus", bus.name().to_lowercase()),
                level,
            )
        }
    }
    fn assign(&mut self, stream: u32, channel: Option<usize>) -> Result<(), String> {
        let audio = audio::snapshot()?;
        let item = audio
            .playback
            .iter()
            .find(|i| i.index == stream && !i.internal)
            .ok_or("This app's audio stream has ended")?;
        let current = audio
            .outputs
            .iter()
            .find(|o| Some(o.index) == item.target)
            .ok_or("The app's output device disappeared")?;
        if let Some(i) = channel {
            if i >= 4 {
                return Err("Applications must use Music, Game, Voice, or Browser.".into());
            }
            let target = format!("{PREFIX}{}", KEYS[i]);
            if !current.name.starts_with(PREFIX) {
                self.routes.insert(stream, current.name.clone());
            }
            move_stream(stream, &target, &audio)?;
        } else {
            if current.name.starts_with(PREFIX) {
                let target = self.return_target(stream, &audio);
                move_stream(stream, &target, &audio)?;
            }
            self.routes.remove(&stream);
        }
        Ok(())
    }
    fn return_target(&self, stream: u32, audio: &Snapshot) -> String {
        [
            self.routes.get(&stream).map(String::as_str),
            Some(self.original_output.as_str()),
            Some(self.monitor.as_str()),
        ]
        .into_iter()
        .flatten()
        .find(|name| audio.outputs.iter().any(|d| d.name == *name && !d.internal))
        .map(str::to_owned)
        .unwrap_or_else(|| "@DEFAULT_SINK@".into())
    }
    fn restore_defaults(&self, audio: &Snapshot) -> Result<(), String> {
        if audio.default_output.starts_with(PREFIX)
            && audio.outputs.iter().any(|d| d.name == self.original_output)
        {
            run(&["set-default-sink", &self.original_output])?;
        }
        if audio.default_input.starts_with(PREFIX)
            && audio.inputs.iter().any(|d| d.name == self.original_input)
        {
            run(&["set-default-source", &self.original_input])?;
        }
        Ok(())
    }
    fn healthy(&self, audio: &Snapshot) -> bool {
        KEYS.iter().all(|k| {
            audio
                .outputs
                .iter()
                .any(|i| i.name == format!("{PREFIX}{k}"))
        }) && ["listen_bus", "stream_bus"].iter().all(|k| {
            audio
                .outputs
                .iter()
                .any(|i| i.name == format!("{PREFIX}{k}"))
        }) && self.connection_modules.iter().all(|module| {
            audio
                .playback
                .iter()
                .any(|i| i.owner_module == Some(*module))
        }) && audio.outputs.iter().any(|i| i.name == self.monitor)
            && audio.inputs.iter().any(|i| i.name == SOURCE)
            && self
                .microphone
                .as_ref()
                .is_none_or(|mic| audio.inputs.iter().any(|i| &i.name == mic))
    }
    fn stop(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Ok(audio) = audio::snapshot() {
            for item in audio.playback.iter().filter(|i| !i.internal) {
                if audio
                    .outputs
                    .iter()
                    .any(|d| Some(d.index) == item.target && d.name.starts_with(PREFIX))
                    && let Err(e) =
                        move_stream(item.index, &self.return_target(item.index, &audio), &audio)
                {
                    errors.push(e);
                }
            }
            if let Err(e) = self.restore_defaults(&audio) {
                errors.push(e);
            }
        }
        // Module indices can be reused after a server restart. Only remove modules bearing
        // this session's unique token; never unload a module based on its saved id alone.
        // PipeWire 1.6's JSON module list omits indices; the short table includes them.
        let owned = owned_module_ids(&run(&["list", "short", "modules"])?, &self.token);
        self.modules.retain(|id| owned.contains(id));
        for id in self.modules.clone().into_iter().rev() {
            match run(&["unload-module", &id.to_string()]) {
                Ok(_) => self.modules.retain(|i| *i != id),
                Err(e) => errors.push(e),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

fn move_stream(stream: u32, target: &str, audio: &Snapshot) -> Result<(), String> {
    let name = if target == "@DEFAULT_SINK@" {
        audio.default_output.as_str()
    } else {
        target
    };
    let destination = audio
        .outputs
        .iter()
        .find(|i| i.name == name)
        .ok_or("The destination output is unavailable")?
        .index;
    run(&["move-sink-input", &stream.to_string(), target])?;
    // PipeWire acknowledges a move before WirePlumber has relinked the stream. In particular,
    // removing its old sink immediately can override the move with the system default.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let inputs = audio::parse_items(
            &run(&["--format=json", "list", "sink-inputs"])?,
            Kind::Playback,
        )?;
        if inputs
            .iter()
            .find(|i| i.index == stream)
            .is_none_or(|i| i.target == Some(destination))
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("The audio server did not finish routing the app.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn owned_module_ids(table: &str, token: &str) -> BTreeSet<u32> {
    table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?.parse().ok()?;
            let _name = fields.next()?;
            let arguments = fields.next()?;
            arguments.contains(token).then_some(id)
        })
        .collect()
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.modules.is_empty()
            && let Err(error) = self.stop()
        {
            eprintln!("Stream mixer cleanup: {error}");
        }
    }
}

fn set_level(kind: Kind, id: String, level: Level) -> Result<(), String> {
    audio::pactl(&[
        format!("set-{}-volume", kind.entity()),
        id.clone(),
        format!("{}%", level.volume.min(100)),
    ])?;
    audio::pactl(&[
        format!("set-{}-mute", kind.entity()),
        id,
        u8::from(level.muted).to_string(),
    ])?;
    Ok(())
}

fn handle(
    session: &mut Option<Session>,
    state: &mut MixState,
    action: &MixAction,
) -> Result<(), String> {
    match action {
        MixAction::Start {
            monitor,
            microphone,
        } => {
            if session.is_some() {
                return Err("The stream mixer is already running.".into());
            }
            *session = Some(Session::start(state, monitor, microphone.as_deref())?);
        }
        MixAction::Stop => {
            if let Some(graph) = session {
                graph.stop()?;
            }
            session.take();
            state.active = false;
            state.sinks = [None; 5];
        }
        MixAction::Level {
            channel,
            bus,
            level,
        } => {
            if channel.is_some_and(|i| i >= CHANNELS.len()) {
                return Err("Unknown mix channel".into());
            }
            if let Some(graph) = session {
                graph.level(*channel, *bus, *level)?;
            }
            state.preview(action);
        }
        MixAction::Assign { stream, channel } => session
            .as_mut()
            .ok_or("Start the mixer before assigning apps")?
            .assign(*stream, *channel)?,
        MixAction::Inspect => {
            if let Some(graph) = session
                && !graph.healthy(&audio::snapshot()?)
            {
                graph.stop()?;
                session.take();
                state.active = false;
                state.sinks = [None; 5];
                return Err("Stream mixer stopped because an audio device or mix connection disappeared. Reconnect the device and start again.".into());
            }
        }
    }
    Ok(())
}

pub fn worker() {
    let mut session = None;
    let mut state = MixState::default();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let error = match serde_json::from_str::<MixAction>(&line) {
            Ok(action) => handle(&mut session, &mut state, &action).err(),
            Err(e) => Some(format!("Invalid mixer command: {e}")),
        };
        let reply = Reply {
            state: state.clone(),
            error,
        };
        if serde_json::to_writer(&mut output, &reply).is_err()
            || output
                .write_all(b"\n")
                .and_then(|_| output.flush())
                .is_err()
        {
            break;
        }
    }
    // Dropping the session restores routes before virtual devices go away.
}

pub fn public_item(item: &Item) -> bool {
    !item.internal
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_levels_preserve_the_other_mix_and_microphone_starts_unmonitored() {
        let mut state = MixState::default();
        assert!(state.levels[4][0].muted);
        assert!(!state.levels[4][1].muted);
        state.preview(&MixAction::Level {
            channel: Some(0),
            bus: Bus::Stream,
            level: Level {
                volume: 140,
                muted: true,
            },
        });
        assert_eq!(
            state.levels[0][1],
            Level {
                volume: 100,
                muted: true
            }
        );
        assert_eq!(
            state.levels[0][0],
            Level {
                volume: 70,
                muted: false
            }
        );
    }
    #[test]
    fn invalid_channels_are_rejected_before_any_audio_command() {
        assert!(
            handle(
                &mut None,
                &mut MixState::default(),
                &MixAction::Level {
                    channel: Some(9),
                    bus: Bus::Listen,
                    level: Level {
                        volume: 0,
                        muted: false
                    }
                }
            )
            .is_err()
        );
    }
    #[test]
    fn pulse_properties_are_quoted_for_the_module_parser() {
        assert_eq!(
            arg("sink_properties", "device.description=\"Oto Oto Music\""),
            "sink_properties=\"device.description=\\\"Oto Oto Music\\\"\""
        );
        assert_eq!(number(&serde_json::json!("536870918")), Some(536870918));
    }

    #[test]
    fn cleanup_only_uses_module_ids_with_this_sessions_token() {
        let table = "1\tmodule-null-sink\toto.session=ours\t\n2\tmodule-null-sink\toto.session=other\t\n3\tnative-module\t\t\n";
        assert_eq!(owned_module_ids(table, "ours"), BTreeSet::from([1]));
    }
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Kind {
    Output,
    Input,
    Playback,
    Recording,
}

impl Kind {
    pub fn entity(self) -> &'static str {
        match self {
            Self::Output => "sink",
            Self::Input => "source",
            Self::Playback => "sink-input",
            Self::Recording => "source-output",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Port {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub availability: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    pub index: u32,
    pub name: String,
    pub title: String,
    pub detail: String,
    pub volume: u32,
    pub muted: bool,
    pub running: bool,
    pub target: Option<u32>,
    pub ports: Vec<Port>,
    pub active_port: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub server: String,
    pub default_output: String,
    pub default_input: String,
    pub outputs: Vec<Item>,
    pub inputs: Vec<Item>,
    pub playback: Vec<Item>,
    pub recording: Vec<Item>,
}

impl Snapshot {
    pub fn items(&self, kind: Kind) -> &[Item] {
        match kind {
            Kind::Output => &self.outputs,
            Kind::Input => &self.inputs,
            Kind::Playback => &self.playback,
            Kind::Recording => &self.recording,
        }
    }
    pub fn items_mut(&mut self, kind: Kind) -> &mut Vec<Item> {
        match kind {
            Kind::Output => &mut self.outputs,
            Kind::Input => &mut self.inputs,
            Kind::Playback => &mut self.playback,
            Kind::Recording => &mut self.recording,
        }
    }
    pub fn default_item(&self, kind: Kind) -> Option<&Item> {
        let name = if kind == Kind::Input {
            &self.default_input
        } else {
            &self.default_output
        };
        self.items(kind).iter().find(|d| &d.name == name)
    }
}

#[derive(Deserialize, Default)]
struct Channel {
    value: u32,
}

#[derive(Deserialize)]
struct RawItem {
    index: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    corked: bool,
    #[serde(default)]
    volume: BTreeMap<String, Channel>,
    #[serde(default)]
    properties: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    sink: Option<u32>,
    #[serde(default)]
    source: Option<u32>,
    #[serde(default)]
    ports: Vec<Port>,
    #[serde(default)]
    active_port: String,
}

pub fn parse_items(json: &str, kind: Kind) -> Result<Vec<Item>, String> {
    let raw: Vec<RawItem> = serde_json::from_str(json)
        .map_err(|e| format!("Invalid {} response: {e}", kind.entity()))?;
    Ok(raw
        .into_iter()
        .filter_map(|r| {
            let prop = |key: &str| {
                r.properties
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            // Monitor sources duplicate outputs; they are not microphone devices.
            if kind == Kind::Input
                && (prop("device.class") == "monitor" || r.name.ends_with(".monitor"))
            {
                return None;
            }
            let stream = matches!(kind, Kind::Playback | Kind::Recording);
            let title = if stream {
                [
                    prop("application.name"),
                    prop("application.process.binary"),
                    prop("node.description"),
                    prop("media.name"),
                ]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or_else(|| "Audio stream".into())
            } else if r.description.is_empty() {
                r.name.clone()
            } else {
                r.description.clone()
            };
            // Use PulseAudio's integer volume, never locale-dependent percentage strings.
            let volume = if r.volume.is_empty() {
                0
            } else {
                (r.volume.values().map(|c| c.value as f64).sum::<f64>()
                    / r.volume.len() as f64
                    / 65536.0
                    * 100.0)
                    .round() as u32
            };
            let detail = if stream {
                prop("media.name")
            } else {
                r.ports
                    .iter()
                    .find(|p| p.name == r.active_port)
                    .map(|p| p.description.clone())
                    .unwrap_or_else(|| "Audio device".into())
            };
            Some(Item {
                index: r.index,
                name: r.name,
                title,
                detail,
                volume,
                muted: r.mute,
                running: if stream {
                    !r.corked
                } else {
                    r.state == "RUNNING"
                },
                target: if kind == Kind::Recording {
                    r.source
                } else {
                    r.sink
                },
                ports: r.ports,
                active_port: r.active_port,
            })
        })
        .collect())
}

// A disconnected audio server must not hang the UI or keep a worker alive forever.
fn pactl(args: &[String]) -> Result<String, String> {
    let mut child = Command::new("pactl").args(args).env("LC_ALL", "C")
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("Cannot run pactl: {e}. Install libpulse (Arch) or pulseaudio-utils (Debian/Ubuntu)."))?;
    // Drain both pipes while waiting; a device list can exceed the OS pipe buffer.
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let _ = std::io::BufReader::new(stdout).read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < Duration::from_secs(3) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Audio server did not respond within 3 seconds.".into());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("Cannot wait for pactl: {e}"));
            }
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if status?.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "pactl {}: {}",
            args.first().map(String::as_str).unwrap_or(""),
            stderr.trim()
        ))
    }
}

pub fn snapshot() -> Result<Snapshot, String> {
    let info: serde_json::Value =
        serde_json::from_str(&pactl(&["--format=json".into(), "info".into()])?)
            .map_err(|e| e.to_string())?;
    let list = |name: &str, kind| {
        parse_items(
            &pactl(&["--format=json".into(), "list".into(), name.into()])?,
            kind,
        )
    };
    Ok(Snapshot {
        server: info["server_name"].as_str().unwrap_or("PulseAudio").into(),
        default_output: info["default_sink_name"]
            .as_str()
            .unwrap_or_default()
            .into(),
        default_input: info["default_source_name"]
            .as_str()
            .unwrap_or_default()
            .into(),
        outputs: list("sinks", Kind::Output)?,
        inputs: list("sources", Kind::Input)?,
        playback: list("sink-inputs", Kind::Playback)?,
        recording: list("source-outputs", Kind::Recording)?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Volume(Kind, u32, u32),
    Mute(Kind, u32, bool),
    Default(Kind, String),
    Port(Kind, u32, String),
    Route(Kind, u32, u32),
    Refresh,
}

impl Action {
    pub fn args(&self) -> Vec<String> {
        match self {
            Self::Volume(k, id, value) => vec![
                format!("set-{}-volume", k.entity()),
                id.to_string(),
                format!("{}%", value.min(&100)),
            ],
            Self::Mute(k, id, muted) => vec![
                format!("set-{}-mute", k.entity()),
                id.to_string(),
                u8::from(*muted).to_string(),
            ],
            Self::Default(k, name) => vec![format!("set-default-{}", k.entity()), name.clone()],
            Self::Port(k, id, name) => vec![
                format!("set-{}-port", k.entity()),
                id.to_string(),
                name.clone(),
            ],
            Self::Route(k, id, target) => vec![
                format!("move-{}", k.entity()),
                id.to_string(),
                target.to_string(),
            ],
            Self::Refresh => vec![],
        }
    }
}

pub struct Update {
    pub revision: u64,
    pub snapshot: Result<Snapshot, String>,
    pub action_error: Option<String>,
}

pub struct Backend {
    pub commands: Sender<(u64, Action)>,
    pub updates: Receiver<Update>,
}

impl Backend {
    pub fn start(waker: rune::Waker) -> Self {
        let (commands, requests) = mpsc::channel::<(u64, Action)>();
        let (updates, results) = mpsc::channel();
        std::thread::spawn(move || {
            let mut pending: Option<(u64, Action)> = None;
            let mut revision = 0;
            loop {
                let mut error = None;
                if let Some((rev, action)) = pending.take() {
                    revision = rev;
                    if action != Action::Refresh {
                        error = pactl(&action.args()).err();
                    }
                    // Apply queued actions before fetching so older snapshots never undo them.
                    while let Ok((rev, action)) = requests.try_recv() {
                        revision = rev;
                        if action != Action::Refresh {
                            error = pactl(&action.args()).err().or(error);
                        }
                    }
                }
                if updates
                    .send(Update {
                        revision,
                        snapshot: snapshot(),
                        action_error: error,
                    })
                    .is_err()
                {
                    break;
                }
                waker();
                match requests.recv_timeout(Duration::from_secs(2)) {
                    Ok(action) => pending = Some(action),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            commands,
            updates: results,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_uses_raw_values_and_monitor_sources_are_hidden() {
        let json = r#"[{"index":1,"name":"mic","volume":{"left":{"value":32768},"right":{"value":65536}},"properties":{"device.class":"sound"}}, {"index":2,"name":"speaker.monitor","properties":{}}]"#;
        let items = parse_items(json, Kind::Input).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].volume, 75);
    }

    #[test]
    fn stream_names_fall_back_and_non_string_properties_are_safe() {
        let items = parse_items(r#"[{"index":7,"sink":3,"properties":{"application.name":42,"application.process.binary":"music","media.name":"A song"}}]"#, Kind::Playback).unwrap();
        assert_eq!(items[0].title, "music");
        assert_eq!(items[0].target, Some(3));
        assert_eq!(items[0].detail, "A song");
    }

    #[test]
    fn commands_are_separate_arguments_and_volume_is_capped() {
        assert_eq!(
            Action::Volume(Kind::Output, 8, 150).args(),
            ["set-sink-volume", "8", "100%"]
        );
        assert_eq!(
            Action::Default(Kind::Input, "a name; with spaces".into()).args(),
            ["set-default-source", "a name; with spaces"]
        );
        assert_eq!(
            Action::Route(Kind::Recording, 4, 9).args(),
            ["move-source-output", "4", "9"]
        );
    }

    #[test]
    fn malformed_server_response_is_an_error() {
        assert!(parse_items("not json", Kind::Output).is_err());
    }
}

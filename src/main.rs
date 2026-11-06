mod audio;
mod ui;

use audio::{Action, Backend, Item, Kind, Port, Snapshot};
use rune::{AppSpec, Key, KeyEvent, Tx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Output,
    Input,
    Apps,
}

struct State {
    audio: Snapshot,
    backend: Option<Backend>,
    tab: Tab,
    demo: bool,
    connected: bool,
    error: Option<String>,
    menu: Option<String>,
    help: bool,
    drag: Option<(Kind, u32)>,
    revision: u64,
    accent: rune::Color,
    next_theme_check: f64,
}

impl State {
    fn new() -> Self {
        // Rune's scripted rendering must never change the user's real audio settings.
        let demo = rune::is_headless() || std::env::args().any(|s| s == "--demo");
        let mut state = Self {
            audio: Snapshot::default(),
            backend: None,
            tab: Tab::Output,
            demo,
            connected: false,
            error: None,
            menu: None,
            help: false,
            drag: None,
            revision: 0,
            accent: rgb(0xffd60a),
            next_theme_check: 0.0,
        };
        if demo {
            state.audio = demo_audio();
            state.connected = true;
        }
        state
    }

    fn action(&mut self, action: Action) {
        match &action {
            Action::Volume(k, id, v) => {
                if let Some(item) = self.item_mut(*k, *id) {
                    item.volume = (*v).min(100);
                }
            }
            Action::Mute(k, id, m) => {
                if let Some(item) = self.item_mut(*k, *id) {
                    item.muted = *m;
                }
            }
            Action::Default(Kind::Output, name) => self.audio.default_output = name.clone(),
            Action::Default(Kind::Input, name) => self.audio.default_input = name.clone(),
            Action::Port(k, id, port) => {
                if let Some(item) = self.item_mut(*k, *id) {
                    item.active_port = port.clone();
                    if let Some(p) = item.ports.iter().find(|p| p.name == *port) {
                        item.detail = p.description.clone();
                    }
                }
            }
            Action::Route(k, id, target) => {
                if let Some(item) = self.item_mut(*k, *id) {
                    item.target = Some(*target);
                }
            }
            _ => {}
        }
        if let Some(backend) = &self.backend {
            self.revision += 1;
            if backend.commands.send((self.revision, action)).is_err() {
                self.error = Some("Audio worker stopped. Restart Oto Oto to reconnect.".into());
                self.connected = false;
            }
        }
    }

    fn item_mut(&mut self, kind: Kind, id: u32) -> Option<&mut Item> {
        self.audio
            .items_mut(kind)
            .iter_mut()
            .find(|d| d.index == id)
    }

    fn mute(&mut self, kind: Kind, id: u32) {
        if let Some(item) = self.audio.items(kind).iter().find(|d| d.index == id) {
            self.action(Action::Mute(kind, id, !item.muted));
        }
    }

    fn volume(&mut self, kind: Kind, id: u32, delta: i32) {
        if let Some(item) = self.audio.items(kind).iter().find(|d| d.index == id) {
            self.action(Action::Volume(
                kind,
                id,
                (item.volume as i32 + delta).clamp(0, 100) as u32,
            ));
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.menu = None;
    }
}

fn setup(s: &mut State, waker: rune::Waker) {
    if !s.demo {
        s.backend = Some(Backend::start(waker));
    }
}

fn poll(s: &mut State, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(backend) = &s.backend {
        while let Ok(update) = backend.updates.try_recv() {
            if let Some(error) = update.action_error {
                s.error = Some(error);
                changed = true;
            }
            if update.revision < s.revision || s.drag.is_some() {
                continue;
            }
            match update.snapshot {
                Ok(audio) => {
                    changed |= s.audio != audio || !s.connected;
                    s.audio = audio;
                    s.connected = true;
                }
                Err(error) => {
                    changed |= s.error.as_ref() != Some(&error) || s.connected;
                    s.error = Some(error);
                    s.connected = false;
                }
            }
        }
    }
    if !s.demo && clock >= s.next_theme_check {
        let accent = read_accent();
        changed |= accent != s.accent;
        s.accent = accent;
        s.next_theme_check = clock + 2.0;
    }
    if !s.demo {
        tx.wake_after(std::time::Duration::from_secs(2));
    }
    changed
}

fn rgb(value: u32) -> rune::Color {
    rune::color::hex(&format!("#{value:06x}")).expect("valid RGB color")
}

fn read_accent() -> rune::Color {
    if let Some(home) = std::env::var_os("HOME") {
        let path =
            std::path::PathBuf::from(home).join(".local/state/omarchy/current/theme/colors.toml");
        if let Ok(contents) = std::fs::read_to_string(path) {
            for line in contents.lines() {
                if let Some((key, value)) = line.split_once('=')
                    && key.trim() == "accent"
                {
                    let value = value
                        .trim()
                        .trim_matches(['\'', '"'])
                        .trim_start_matches('#');
                    if value.len() == 6
                        && let Ok(value) = u32::from_str_radix(value, 16)
                    {
                        return rgb(value);
                    }
                }
            }
        }
    }
    rgb(0xffd60a)
}

fn on_key(s: &mut State, event: &KeyEvent, tx: &mut Tx) -> bool {
    if let Key::Text(text) = &event.key {
        match text.as_str() {
            "1" => s.switch_tab(Tab::Output),
            "2" => s.switch_tab(Tab::Input),
            "3" => s.switch_tab(Tab::Apps),
            "?" => s.help = !s.help,
            "q" => tx.quit(),
            "r" => s.action(Action::Refresh),
            "m" | " " | "+" | "=" | "-" => {
                let kind = if s.tab == Tab::Input {
                    Kind::Input
                } else {
                    Kind::Output
                };
                if let Some(item) = s.audio.default_item(kind) {
                    let id = item.index;
                    if text == "m" || text == " " {
                        s.mute(kind, id);
                    } else {
                        s.volume(kind, id, if text == "-" { -5 } else { 5 });
                    }
                }
            }
            _ => return false,
        }
        return true;
    }
    if event.key == Key::Escape {
        s.help = false;
        s.menu = None;
        return true;
    }
    false
}

fn demo_audio() -> Snapshot {
    let device = |index, name: &str, title: &str, port: &str, volume, running| Item {
        index,
        name: name.into(),
        title: title.into(),
        detail: port.into(),
        volume,
        muted: false,
        running,
        target: None,
        ports: vec![Port {
            name: "analog".into(),
            description: port.into(),
            availability: "available".into(),
        }],
        active_port: "analog".into(),
    };
    let stream = |index, title: &str, detail: &str, volume, target| Item {
        index,
        name: String::new(),
        title: title.into(),
        detail: detail.into(),
        volume,
        muted: false,
        running: true,
        target: Some(target),
        ports: vec![],
        active_port: String::new(),
    };
    Snapshot {
        server: "PulseAudio (on PipeWire)".into(),
        default_output: "speakers".into(),
        default_input: "mic".into(),
        outputs: vec![
            device(1, "speakers", "Studio speakers", "Analog stereo", 64, true),
            device(2, "headphones", "USB headphones", "USB audio", 42, false),
            device(3, "hdmi", "Display audio", "HDMI / DisplayPort", 75, false),
        ],
        inputs: vec![
            device(4, "mic", "USB microphone", "Mono input", 80, true),
            device(
                5,
                "internal",
                "Built-in microphone",
                "Internal microphone",
                55,
                false,
            ),
        ],
        playback: vec![
            stream(10, "Spotify", "Khruangbin — A Calf Born in Winter", 72, 1),
            stream(11, "Firefox", "Audio playback", 45, 2),
            stream(12, "Discord", "Voice channel", 88, 1),
        ],
        recording: vec![stream(13, "Discord", "Voice input", 100, 4)],
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Oto Oto — Linux audio control\n\n  oto-oto                 Open the mixer\n  oto-oto --demo          Open with sample devices; no audio changes\n  oto-oto status         Print real audio state as JSON\n  oto-oto --snap FILE     Render sample devices to a PNG\n  oto-oto --scene apps    Select the apps scene for rendering\n\nShortcuts: 1/2/3 tabs · M/Space mute default · +/- volume · R refresh · ? help · Q quit"
        );
        return;
    }
    if args.first().is_some_and(|a| a == "status") {
        match audio::snapshot() {
            Ok(snapshot) => println!("{}", serde_json::to_string_pretty(&snapshot).unwrap()),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    rune::set_app_id("oto-oto");
    rune::run(AppSpec {
        min_size: Some((740.0, 560.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        scenes: &[
            ("output", |s| s.tab = Tab::Output),
            ("input", |s| s.tab = Tab::Input),
            ("apps", |s| s.tab = Tab::Apps),
            ("empty", |s| s.audio = Snapshot::default()),
            ("error", |s| {
                s.connected = false;
                s.error = Some("Cannot connect to the audio server. Check that PipeWire or PulseAudio is running, then retry.".into());
            }),
        ],
        ..AppSpec::new("Oto Oto", (940.0, 740.0), State::new, ui::view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};

    fn runtime() -> Runtime<State> {
        let mut s = State::new();
        s.demo = true;
        s.audio = demo_audio();
        s.connected = true;
        let mut rt = Runtime::new(s, ui::view, (940.0, 740.0), Env::new(true, 1.0));
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        rt
    }

    fn settle(rt: &mut Runtime<State>) {
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn click(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt
            .find(key)
            .unwrap_or_else(|| panic!("Missing element: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    #[test]
    fn devices_can_be_muted_and_made_default() {
        let mut rt = runtime();
        click(&mut rt, "mute-sink-1");
        assert!(rt.state.audio.outputs[0].muted);
        click(&mut rt, "default-2");
        assert_eq!(rt.state.audio.default_output, "headphones");
        assert!(rt.find("default-2").is_none());
        assert!(rt.find("default-1").is_some());
    }

    #[test]
    fn slider_drag_can_be_cancelled_and_keyboard_steps_work() {
        let mut rt = runtime();
        let slider = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("volume-sink-1"))
            .unwrap();
        let [x, y, w, h] = slider.rect;
        rt.pointer_down(x + 0.64 * w, y + h / 2.0);
        rt.pointer_move(x + w * 0.9, y + h / 2.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.audio.outputs[0].volume > 80);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.pointer_up(x + w * 0.9, y + h / 2.0);
        settle(&mut rt);
        assert_eq!(rt.state.audio.outputs[0].volume, 64);
        rt.key(KeyEvent {
            key: Key::Right,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.audio.outputs[0].volume, 65);
        rt.key(KeyEvent {
            key: Key::End,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.audio.outputs[0].volume, 100);
    }

    #[test]
    fn app_routing_select_moves_only_the_chosen_stream() {
        let mut rt = runtime();
        click(&mut rt, "tab-Apps");
        click(&mut rt, "route-sink-input-10-trigger");
        assert!(rt.state.menu.is_some());
        click(&mut rt, "option1");
        assert_eq!(rt.state.audio.playback[0].target, Some(2));
        assert_eq!(rt.state.audio.playback[2].target, Some(1));
        assert!(rt.state.menu.is_none());
    }

    #[test]
    fn input_tab_controls_microphones_and_help_closes() {
        let mut rt = runtime();
        click(&mut rt, "tab-Input");
        click(&mut rt, "mute-source-4");
        assert!(rt.state.audio.inputs[0].muted);
        assert!(!rt.state.audio.outputs[0].muted);
        click(&mut rt, "help-button");
        assert!(rt.find("help-overlay").is_some());
        click(&mut rt, "close-help");
        assert!(!rt.state.help);
    }

    #[test]
    fn port_menu_uses_the_right_port_and_hides_unavailable_ports() {
        let mut rt = runtime();
        rt.state.audio.outputs[0].ports.extend([
            Port {
                name: "headphones".into(),
                description: "Headphones".into(),
                availability: "available".into(),
            },
            Port {
                name: "unplugged".into(),
                description: "Disconnected".into(),
                availability: "not available".into(),
            },
        ]);
        rt.invalidate();
        settle(&mut rt);
        click(&mut rt, "port-sink-1-trigger");
        assert!(rt.find("option2").is_none());
        click(&mut rt, "option1");
        assert_eq!(rt.state.audio.outputs[0].active_port, "headphones");
        assert_eq!(rt.state.audio.outputs[0].detail, "Headphones");
    }

    #[test]
    fn stale_worker_snapshots_cannot_undo_a_newer_command() {
        let mut rt = runtime();
        let (commands, requests) = std::sync::mpsc::channel();
        let (updates, results) = std::sync::mpsc::channel();
        rt.state.backend = Some(Backend {
            commands,
            updates: results,
        });
        rt.state.action(Action::Volume(Kind::Output, 1, 20));
        assert_eq!(
            requests.recv().unwrap(),
            (1, Action::Volume(Kind::Output, 1, 20))
        );
        updates
            .send(audio::Update {
                revision: 0,
                snapshot: Ok(demo_audio()),
                action_error: None,
            })
            .unwrap();
        let mut tx = Tx::default();
        poll(&mut rt.state, 0.0, &mut tx);
        assert_eq!(rt.state.audio.outputs[0].volume, 20);
        let mut fresh = demo_audio();
        fresh.outputs[0].volume = 21;
        updates
            .send(audio::Update {
                revision: 1,
                snapshot: Ok(fresh),
                action_error: None,
            })
            .unwrap();
        poll(&mut rt.state, 0.0, &mut tx);
        assert_eq!(rt.state.audio.outputs[0].volume, 21);
    }
}

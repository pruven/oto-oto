use crate::audio::{Action, Item, Kind};
use crate::{State, Tab};
use rune::{
    Align, Color, Cx, Drag, El, Phase, ScrollSpec, Shape, TextStyle, canvas, col, el, row, text,
};
use rune_kit::{Kit, PaletteSeed, Select};

const BG: Color = Color {
    r: 0.007499032,
    g: 0.008023193,
    b: 0.008568126,
    a: 1.0,
};
const PANEL: Color = Color {
    r: 0.011612245,
    g: 0.012286488,
    b: 0.013702083,
    a: 1.0,
};
const BORDER: Color = Color {
    r: 0.029556834,
    g: 0.031896033,
    b: 0.035601315,
    a: 1.0,
};
const INK: Color = Color {
    r: 0.854_992_6,
    g: 0.822_785_74,
    b: 0.752_942_2,
    a: 1.0,
};
const MUTED: Color = Color {
    r: 0.351_532_6,
    g: 0.351_532_6,
    b: 0.327_778_1,
    a: 1.0,
};
const FAINT: Color = Color {
    r: 0.184_474_99,
    g: 0.191_201_69,
    b: 0.181_164_25,
    a: 1.0,
};

fn label(value: impl Into<String>, size: f32, weight: f32, color: Color) -> El {
    text(value).font(TextStyle::new(size, weight)).color(color)
}

fn line() -> El {
    el().w_full().h(1.0).fill(BORDER).a11y_hidden()
}

fn button(value: &str) -> El {
    row()
        .role("button")
        .label(value)
        .focusable(true)
        .center()
        .px(12.0)
        .h(30.0)
        .border(1.0, BORDER)
        .hover(INK.alpha(0.06))
        .child(label(value, 11.0, 500.0, INK))
}

#[derive(Clone, Copy)]
enum Glyph {
    Output,
    Input,
    Apps,
    Muted,
}

fn glyph(kind: Glyph, color: Color, size: f32) -> El {
    let s = Shape::new();
    let s = match kind {
        Glyph::Output | Glyph::Muted => {
            let s = s.stroke(
                vec![
                    [3., 9.],
                    [7., 9.],
                    [12., 5.],
                    [12., 19.],
                    [7., 15.],
                    [3., 15.],
                    [3., 9.],
                ],
                1.5,
                color,
            );
            if matches!(kind, Glyph::Muted) {
                s.stroke(vec![[17., 9.], [22., 15.]], 1.5, color).stroke(
                    vec![[22., 9.], [17., 15.]],
                    1.5,
                    color,
                )
            } else {
                s.stroke(
                    vec![[16., 8.], [18., 10.], [18.5, 12.], [18., 14.], [16., 16.]],
                    1.5,
                    color,
                )
                .stroke(
                    vec![[19., 4.], [22., 8.], [23., 12.], [22., 16.], [19., 20.]],
                    1.5,
                    color,
                )
            }
        }
        Glyph::Input => s
            .stroke(
                vec![
                    [9., 4.],
                    [12., 2.],
                    [15., 4.],
                    [15., 12.],
                    [12., 14.],
                    [9., 12.],
                    [9., 4.],
                ],
                1.5,
                color,
            )
            .stroke(
                vec![
                    [5., 10.],
                    [5., 14.],
                    [8., 18.],
                    [16., 18.],
                    [19., 14.],
                    [19., 10.],
                ],
                1.5,
                color,
            )
            .stroke(vec![[12., 18.], [12., 22.]], 1.5, color)
            .stroke(vec![[8., 22.], [16., 22.]], 1.5, color),
        Glyph::Apps => s
            .stroke(vec![[5., 8.], [5., 16.]], 1.8, color)
            .stroke(vec![[12., 3.], [12., 21.]], 1.8, color)
            .stroke(vec![[19., 7.], [19., 17.]], 1.8, color),
    };
    canvas(s.view_box(24.0, 24.0))
        .size(size, size)
        .a11y_hidden()
}

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut kit = Kit::from_palette(PaletteSeed {
        bg: BG,
        fg: INK,
        accent: s.accent,
    });
    kit.radius.sm = 0.0;
    kit.radius.md = 0.0;
    let mut layers = Vec::new();
    let mut nav = row()
        .role("tablist")
        .label("Audio controls")
        .focus_group()
        .focus_follows()
        .w_full()
        .gap(26.0);
    for (tab, title, icon, count) in [
        (Tab::Output, "Output", Glyph::Output, s.audio.outputs.len()),
        (Tab::Input, "Input", Glyph::Input, s.audio.inputs.len()),
        (
            Tab::Apps,
            "Applications",
            Glyph::Apps,
            s.audio.playback.len() + s.audio.recording.len(),
        ),
    ] {
        let active = s.tab == tab;
        nav = nav.child(
            col()
                .key(format!("tab-{tab:?}"))
                .role("tab")
                .label(title)
                .focus_selected(active)
                .focusable(true)
                .on_press(move |s: &mut State| s.switch_tab(tab))
                .child(
                    row()
                        .h(49.0)
                        .gap(9.0)
                        .center_y()
                        .child(glyph(icon, if active { s.accent } else { MUTED }, 17.0))
                        .child(label(title, 13.0, 500.0, if active { INK } else { MUTED }))
                        .child(label(format!("{count:02}"), 10.0, 500.0, FAINT)),
                )
                .child(
                    el().w_full()
                        .h(2.0)
                        .fill(if active { s.accent } else { BG }),
                ),
        );
    }
    let (heading, subheading) = match s.tab {
        Tab::Output => (
            "Sound, where you want it.",
            "Choose an output and set a comfortable level.",
        ),
        Tab::Input => (
            "Make yourself heard.",
            "Choose a microphone and adjust its input level.",
        ),
        Tab::Apps => (
            "Every app. Its own space.",
            "Mix playback and choose where each app sends its sound.",
        ),
    };
    let mut content = col().key("content").w_full().gap(12.0).child(
        col()
            .gap(8.0)
            .pt(26.0)
            .pb(16.0)
            .child(label(heading, 28.0, 500.0, INK))
            .child(label(subheading, 12.0, 400.0, MUTED)),
    );

    if let Some(error) = &s.error {
        content = content.child(
            col()
                .key("error")
                .role("alert")
                .w_full()
                .padding(14.0)
                .gap(10.0)
                .border(1.0, crate::rgb(0x80513d))
                .fill(crate::rgb(0x2c231e))
                .child(label(error, 12.0, 400.0, crate::rgb(0xf4c4a4)))
                .child(
                    row()
                        .gap(8.0)
                        .child(
                            button("Retry")
                                .key("retry")
                                .on_press(|s: &mut State| s.action(Action::Refresh)),
                        )
                        .child(button("Dismiss").on_press(|s: &mut State| s.error = None)),
                ),
        );
    }

    match s.tab {
        Tab::Output | Tab::Input => {
            let kind = if s.tab == Tab::Output {
                Kind::Output
            } else {
                Kind::Input
            };
            let default_name = if kind == Kind::Output {
                &s.audio.default_output
            } else {
                &s.audio.default_input
            };
            let items = s.audio.items(kind);
            content = content.child(section_title(
                if kind == Kind::Output {
                    "OUTPUT DEVICES"
                } else {
                    "INPUT DEVICES"
                },
                &format!("{} connected", items.len()),
            ));
            if items.is_empty() {
                content = content.child(empty(
                    if s.connected {
                        "No devices found"
                    } else {
                        "Waiting for the audio server"
                    },
                    "Connected devices will appear here automatically.",
                ));
            }
            for item in items {
                content = content.child(device_card(
                    s,
                    &kit,
                    &mut layers,
                    kind,
                    item,
                    &item.name == default_name,
                ));
            }
            content = content.child(
                label(
                    "Default applies to new streams. Move an existing stream in Applications.",
                    11.0,
                    400.0,
                    FAINT,
                )
                .pt(6.0),
            );
        }
        Tab::Apps => {
            for (kind, title, items) in [
                (Kind::Playback, "PLAYBACK", &s.audio.playback),
                (Kind::Recording, "RECORDING", &s.audio.recording),
            ] {
                content = content.child(section_title(title, &format!("{} streams", items.len())));
                if items.is_empty() {
                    content = content.child(empty(
                        "No active streams",
                        if kind == Kind::Playback {
                            "Play something in an app to see it here."
                        } else {
                            "Apps using a microphone will appear here."
                        },
                    ));
                }
                for item in items {
                    content = content.child(stream_card(s, &kit, &mut layers, kind, item));
                }
            }
        }
    }
    let footer = row()
        .w_full()
        .h(44.0)
        .center_y()
        .gap(7.0)
        .px(28.0)
        .fill(PANEL)
        .child(
            el().size(5.0, 5.0)
                .radius(2.5)
                .fill(if s.connected { s.accent } else { MUTED }),
        )
        .child(label(
            if s.demo {
                "Sample devices · demo mode"
            } else if s.connected {
                if s.audio.server.contains("PipeWire") {
                    "Connected to PipeWire"
                } else {
                    "Connected to PulseAudio"
                }
            } else {
                "Audio server disconnected"
            },
            10.0,
            400.0,
            MUTED,
        ))
        .child(el().grow(1.0))
        .child(label("1–3 switch tabs", 10.0, 400.0, FAINT))
        .child(
            button("?  Shortcuts")
                .key("help-button")
                .on_press(|s: &mut State| s.help = !s.help),
        );

    let body = col()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(BG)
        .child(
            row()
                .w_full()
                .h(78.0)
                .px(28.0)
                .center_y()
                .gap(13.0)
                .child(row().size(34.0, 34.0).fill(s.accent).center().child(glyph(
                    Glyph::Apps,
                    BG,
                    22.0,
                )))
                .child(
                    col()
                        .gap(3.0)
                        .child(label("oto oto", 19.0, 600.0, INK))
                        .child(label("AUDIO CONTROL", 8.0, 600.0, MUTED)),
                )
                .child(el().grow(1.0))
                .child(label("Your sound. In balance.", 11.0, 400.0, FAINT)),
        )
        .child(line())
        .child(col().px(28.0).child(nav).child(line()))
        .child(
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(content.px(28.0).pb(24.0)),
        )
        .child(line())
        .child(footer);

    if s.help {
        layers.push(el().key("help-overlay").absolute(0.0, 0.0).w_full().h_full().fill(BG.alpha(0.9)).center()
            .child(col().w(460.0).padding(28.0).gap(16.0).fill(PANEL).border(1.0, BORDER)
                .child(label("At your fingertips", 22.0, 500.0, INK))
                .child(label("1 / 2 / 3     Output / Input / Applications\nM or Space    Mute the default device\n+ / −         Change default volume by 5%\nR             Refresh devices\nTab           Move between controls\nArrow keys    Adjust a focused slider\n? / Escape    Close this panel\nQ             Quit", 13.0, 400.0, MUTED))
                .child(button("Close").key("close-help").on_press(|s: &mut State| s.help = false)))
            .modal(true, |s: &mut State| s.help = false));
    }
    rune_kit::root(body, layers)
}

fn section_title(title: &str, detail: &str) -> El {
    row()
        .w_full()
        .center_y()
        .pb(3.0)
        .child(label(title, 10.0, 600.0, MUTED))
        .child(el().grow(1.0))
        .child(label(detail, 10.0, 400.0, FAINT))
}

fn empty(title: &str, description: &str) -> El {
    col()
        .w_full()
        .padding(26.0)
        .gap(8.0)
        .border(1.0, BORDER)
        .child(label(title, 15.0, 500.0, INK))
        .child(label(description, 12.0, 400.0, MUTED))
}

fn choice_labels(names: impl Iterator<Item = String>) -> Vec<String> {
    let names: Vec<_> = names.collect();
    let mut used = std::collections::BTreeSet::new();
    names
        .iter()
        .map(|name| {
            let mut label = name.clone();
            if names.iter().filter(|other| *other == name).count() > 1 {
                let mut n = 1;
                loop {
                    label = format!("{name} ({n})");
                    if !names.contains(&label) && !used.contains(&label) {
                        break;
                    }
                    n += 1;
                }
            }
            used.insert(label.clone());
            label
        })
        .collect()
}

fn device_card(
    s: &State,
    kit: &Kit,
    layers: &mut Vec<El>,
    kind: Kind,
    item: &Item,
    default: bool,
) -> El {
    let id = item.index;
    let name = item.name.clone();
    let tag = if default {
        row()
            .h(26.0)
            .center_y()
            .px(10.0)
            .fill(s.accent.alpha(0.1))
            .child(label("DEFAULT", 9.0, 600.0, s.accent))
    } else {
        button("Set default")
            .key(format!("default-{id}"))
            .on_press(move |s: &mut State| s.action(Action::Default(kind, name.clone())))
    };
    let mut card = col()
        .key(format!("device-{}-{id}", kind.entity()))
        .w_full()
        .padding(18.0)
        .gap(16.0)
        .fill(PANEL)
        .border(
            1.0,
            if default {
                s.accent.alpha(0.48)
            } else {
                BORDER
            },
        )
        .child(
            row()
                .w_full()
                .center_y()
                .gap(14.0)
                .child(
                    row()
                        .size(38.0, 38.0)
                        .center()
                        .fill(if default { s.accent.alpha(0.09) } else { BG })
                        .child(glyph(
                            if kind == Kind::Input {
                                Glyph::Input
                            } else {
                                Glyph::Output
                            },
                            if default { s.accent } else { MUTED },
                            21.0,
                        )),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(5.0)
                        .child(label(&item.title, 14.0, 500.0, INK))
                        .child(label(&item.detail, 11.0, 400.0, MUTED)),
                )
                .child(tag),
        )
        .child(volume_control(s, kind, item));
    let ports: Vec<_> = item
        .ports
        .iter()
        .filter(|p| p.availability != "not available")
        .collect();
    if ports.len() > 1 {
        let key = format!("port-{}-{id}", kind.entity());
        let choices = choice_labels(ports.iter().map(|p| p.description.clone()));
        let values: Vec<_> = ports.iter().map(|p| p.name.clone()).collect();
        let current = ports
            .iter()
            .position(|p| p.name == item.active_port)
            .map(|i| choices[i].as_str());
        let toggle = key.clone();
        let labels = choices.clone();
        card = card.child(
            row()
                .w_full()
                .center_y()
                .child(label("PORT", 9.0, 600.0, FAINT))
                .child(el().grow(1.0))
                .child(
                    Select::new(&key, &choices)
                        .label("Device port")
                        .current(current)
                        .open(s.menu.as_ref() == Some(&key))
                        .on_toggle(move |s: &mut State| {
                            if s.menu.as_ref() == Some(&toggle) {
                                s.menu = None;
                            } else {
                                s.menu = Some(toggle.clone());
                            }
                        })
                        .on_close(|s: &mut State| s.menu = None)
                        .on_choose(move |s: &mut State, choice, _| {
                            if let Some(i) = labels.iter().position(|label| label == choice) {
                                s.action(Action::Port(kind, id, values[i].clone()));
                            }
                            s.menu = None;
                        })
                        .build(kit, layers),
                ),
        );
    }
    card
}

fn stream_card(s: &State, kit: &Kit, layers: &mut Vec<El>, kind: Kind, item: &Item) -> El {
    let id = item.index;
    let devices = if kind == Kind::Recording {
        &s.audio.inputs
    } else {
        &s.audio.outputs
    };
    let choices = choice_labels(devices.iter().map(|d| d.title.clone()));
    let ids: Vec<_> = devices.iter().map(|d| d.index).collect();
    let labels = choices.clone();
    let current = devices
        .iter()
        .position(|d| Some(d.index) == item.target)
        .map(|i| choices[i].as_str());
    let key = format!("route-{}-{id}", kind.entity());
    let toggle = key.clone();
    col()
        .key(format!("stream-{}-{id}", kind.entity()))
        .w_full()
        .padding(18.0)
        .gap(14.0)
        .fill(PANEL)
        .border(1.0, BORDER)
        .child(
            row()
                .w_full()
                .gap(12.0)
                .center_y()
                .child(
                    row()
                        .size(34.0, 34.0)
                        .center()
                        .fill(s.accent.alpha(0.1))
                        .child(label(
                            item.title
                                .chars()
                                .next()
                                .unwrap_or('A')
                                .to_uppercase()
                                .to_string(),
                            15.0,
                            600.0,
                            s.accent,
                        )),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(5.0)
                        .child(label(&item.title, 13.0, 500.0, INK))
                        .child(label(&item.detail, 11.0, 400.0, MUTED)),
                )
                .child(label(
                    if item.running { "ACTIVE" } else { "IDLE" },
                    9.0,
                    500.0,
                    FAINT,
                )),
        )
        .child(volume_control(s, kind, item))
        .child(
            row()
                .w_full()
                .center_y()
                .child(label(
                    if kind == Kind::Recording {
                        "RECORD FROM"
                    } else {
                        "PLAY THROUGH"
                    },
                    9.0,
                    600.0,
                    FAINT,
                ))
                .child(el().grow(1.0))
                .child(
                    Select::new(&key, &choices)
                        .label("Stream audio device")
                        .current(current)
                        .placeholder("Choose device")
                        .disabled(devices.is_empty())
                        .open(s.menu.as_ref() == Some(&key))
                        .on_toggle(move |s: &mut State| {
                            if s.menu.as_ref() == Some(&toggle) {
                                s.menu = None;
                            } else {
                                s.menu = Some(toggle.clone());
                            }
                        })
                        .on_close(|s: &mut State| s.menu = None)
                        .on_choose(move |s: &mut State, choice, _| {
                            if let Some(i) = labels.iter().position(|label| label == choice) {
                                s.action(Action::Route(kind, id, ids[i]));
                            }
                            s.menu = None;
                        })
                        .build(kit, layers),
                ),
        )
}

fn volume_control(s: &State, kind: Kind, item: &Item) -> El {
    let id = item.index;
    let v = item.volume.min(100) as f32;
    let w = 292.0;
    let knob = 12.0;
    let travel = w - knob;
    let accent = if item.muted { FAINT } else { s.accent };
    row()
        .w_full()
        .gap(14.0)
        .center_y()
        .child(
            row()
                .key(format!("mute-{}-{id}", kind.entity()))
                .role("button")
                .label(format!(
                    "{} {}",
                    if item.muted { "Unmute" } else { "Mute" },
                    item.title
                ))
                .checked(item.muted)
                .focusable(true)
                .size(28.0, 28.0)
                .center()
                .hover(INK.alpha(0.06))
                .child(glyph(
                    if item.muted {
                        Glyph::Muted
                    } else if kind == Kind::Input || kind == Kind::Recording {
                        Glyph::Input
                    } else {
                        Glyph::Output
                    },
                    if item.muted { FAINT } else { MUTED },
                    18.0,
                ))
                .on_press(move |s: &mut State| s.mute(kind, id)),
        )
        .child(
            el().key(format!("volume-{}-{id}", kind.entity()))
                .role("slider")
                .label(format!("{} volume", item.title))
                .focusable(true)
                .range_value(item.volume as f64, 0.0, item.volume.max(100) as f64)
                .range_step(1.0)
                .a11y_value(format!("{} percent", item.volume))
                .size(w, 28.0)
                .on_step(move |s: &mut State, steps, _| s.volume(kind, id, steps))
                .on_step_limit(move |s: &mut State, end, _| {
                    s.action(Action::Volume(kind, id, if end { 100 } else { 0 }))
                })
                .on_drag(move |s: &mut State, d: &mut Drag, tx| {
                    match d.phase {
                        Phase::Down => {
                            d.data = s
                                .audio
                                .items(kind)
                                .iter()
                                .find(|i| i.index == id)
                                .map(|i| i.volume as f32)
                                .unwrap_or(v);
                            s.drag = Some((kind, id));
                        }
                        Phase::Move => tx.instant(),
                        Phase::Up => {}
                    }
                    let value =
                        (((d.x - knob / 2.0) / travel).clamp(0.0, 1.0) * 100.0).round() as u32;
                    if d.phase == Phase::Up {
                        let value = if d.cancelled() { d.data as u32 } else { value };
                        s.drag = None;
                        if d.cancelled() {
                            if let Some(item) = s.item_mut(kind, id) {
                                item.volume = value;
                            }
                        } else {
                            s.action(Action::Volume(kind, id, value));
                        }
                    } else if let Some(item) = s.item_mut(kind, id) {
                        item.volume = value;
                    }
                })
                .child(
                    el().size(w, 3.0)
                        .absolute(0.0, 12.5)
                        .fill(BORDER)
                        .a11y_hidden(),
                )
                .child(
                    el().size(knob / 2.0 + v / 100.0 * travel, 3.0)
                        .absolute(0.0, 12.5)
                        .fill(accent)
                        .a11y_hidden(),
                )
                .child(
                    el().size(knob, knob)
                        .radius(knob / 2.0)
                        .absolute(v / 100.0 * travel, 8.0)
                        .fill(accent)
                        .a11y_hidden(),
                ),
        )
        .child(
            label(
                format!("{}%", item.volume),
                13.0,
                500.0,
                if item.muted { FAINT } else { INK },
            )
            .w(45.0)
            .align(Align::End),
        )
        .child(el().grow(1.0))
        .child(label(
            if item.muted { "Muted" } else { "" },
            10.0,
            400.0,
            FAINT,
        ))
}

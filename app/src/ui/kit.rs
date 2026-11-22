//! Small building blocks shared by the screens.

use alloy_ui::prelude::*;

use crate::app::{Kahf, TestState};

pub type A = Act<Kahf>;
pub type V = Node<A>;

/// A labelled control: small label above, optional hint below.
pub fn lab(label: &str, control: impl Into<V>) -> V {
    col![caption(label).dim(), control.into()].gap(4.0).into()
}

pub fn lab_hint(label: &str, control: impl Into<V>, hint: &str) -> V {
    col![caption(label).dim(), control.into(), caption(hint).dim()]
        .gap(4.0)
        .into()
}

/// Word-wrap `body` into lines of at most `width_chars` characters. A word longer than the
/// width gets a line of its own.
pub fn wrap_lines(body: &str, width_chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in body.split_whitespace() {
        let need = cur.chars().count() + usize::from(!cur.is_empty()) + word.chars().count();
        if !cur.is_empty() && need > width_chars {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// A long paragraph. Alloy does not count the extra lines of wrapped text when laying out
/// what follows, so we wrap it ourselves: one small dim line per widget, so every line has height.
pub fn para(body: &str, width_chars: usize) -> V {
    let mut c = col![].gap(2.0);
    for line in wrap_lines(body, width_chars) {
        c = c.push(text(line).caption().dim());
    }
    c.into()
}

/// A boxed note. `warn` makes it stand out.
pub fn note(body: impl Into<String>, warn: bool) -> V {
    let t = text(body).caption();
    let t = if warn { t.accent() } else { t.dim() };
    col![t].pad(10.0).round(8.0).outline().fill_w().into()
}

/// The big radio-style choice: title, small explanation, highlighted when chosen.
pub fn choice(selected: bool, title: &str, sub: &str, on: impl Fn(&mut Kahf) + Send + Sync + 'static) -> V {
    let mut c = col![button(title).nav(selected).fill_w().act(on)].gap(2.0);
    if !sub.is_empty() {
        c = c.push(para(sub, 60));
    }
    c.fill_w().into()
}

/// A Test button with its answer beside it. The tests only listen or look; they never transmit.
pub fn test_button(
    k: &Kahf,
    key: &str,
    label: &str,
    run: impl Fn(&mut Kahf, &str) + Send + Sync + 'static,
) -> V {
    let state = k.ui.tests.get(key).cloned();
    let running = matches!(state, Some(TestState::Running));
    let key_owned = key.to_string();
    let b = button(if running {
        "Testing…".to_string()
    } else {
        label.to_string()
    })
    .enabled(!running)
    .act(move |k: &mut Kahf| run(k, &key_owned));
    let mut r = row![b].gap(10.0).align_center();
    if let Some(TestState::Done { ok, text: t }) = state {
        let a = text(t).caption();
        r = r.push(if ok { a.success() } else { a.danger() });
    }
    r.into()
}

/// A section heading with a rule under it.
pub fn titled(title: &str, body: impl Into<V>) -> V {
    col![heading(title), body.into()].gap(8.0).into()
}

/// A dropdown that opens in place. Alloy's own `pick` opens a floating list, and inside a
/// scrolling page that list cannot be clicked, so this one pushes the content below it down.
/// `key` names it, so only one is open at a time.
pub fn dropdown(
    k: &Kahf,
    key: &'static str,
    options: Vec<String>,
    selected: Option<usize>,
    on: impl Fn(&mut Kahf, usize) + Send + Sync + 'static,
) -> V {
    let open = k.ui.open_drop == Some(key);
    let current = selected
        .and_then(|i| options.get(i).cloned())
        .unwrap_or_else(|| "Choose…".into());
    let head = button(format!("{current}   {}", if open { "▴" } else { "▾" }))
        .fill_w()
        .act(move |k: &mut Kahf| {
            k.ui.open_drop = if k.ui.open_drop == Some(key) {
                None
            } else {
                Some(key)
            }
        });
    if !open {
        return head.into();
    }
    let on = std::sync::Arc::new(on);
    let mut list = col![].gap(2.0).pad(4.0).fill_w();
    for (i, name) in options.iter().enumerate() {
        let on = on.clone();
        list = list.push(
            button(name.clone())
                .nav(selected == Some(i))
                .fill_w()
                .act(move |k: &mut Kahf| {
                    k.ui.open_drop = None;
                    on(k, i);
                }),
        );
    }
    let body: V = if options.len() > 8 {
        scroll(list).h(260.0).into()
    } else {
        list.into()
    };
    col![head, body.round(8.0).outline().fill_w()].gap(4.0).into()
}

/// A device list as a dropdown with "Computer default" first. Index 0 is the default.
pub fn device_pick(
    k: &Kahf,
    key: &'static str,
    devices: &[String],
    current: &str,
    set: impl Fn(&mut Kahf, String) + Send + Sync + 'static,
) -> V {
    let mut names = vec!["Computer default".to_string()];
    names.extend(devices.iter().cloned());
    let sel = if current.is_empty() {
        Some(0)
    } else {
        devices.iter().position(|d| d == current).map(|i| i + 1)
    };
    let items = devices.to_vec();
    dropdown(k, key, names, sel, move |k: &mut Kahf, i: usize| {
        set(
            k,
            if i == 0 {
                String::new()
            } else {
                items[i - 1].clone()
            },
        )
    })
}

/// A short label that lights up (green, or red for `bad`) while `on`, and is dim otherwise.
pub fn lit(label: &str, on: bool, bad: bool) -> V {
    let t = text(label).caption();
    match (on, bad) {
        (false, _) => t.dim().into(),
        (true, false) => t.success().into(),
        (true, true) => t.danger().into(),
    }
}

/// A one-line text box that writes into the state.
pub fn edit(value: &str, hint: &str, set: impl Fn(&mut Kahf, String) + Send + Sync + 'static) -> V {
    input(value.to_string())
        .hint(hint)
        .act(move |k: &mut Kahf, v: String| set(k, v))
        .into()
}

/// The warning and estimate shown before files are sent.
pub fn file_notice(k: &Kahf, files: &[crate::types::PickedFile]) -> V {
    use crate::util::fmt::*;
    if files.is_empty() {
        return col![].into();
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    let over = total > FILE_LIMIT_BYTES;
    let bps = k.m.estimate_bps();
    let line = if over {
        format!(
            "{} to send. That is over the {} limit. Radio is slow, so larger files are not accepted.",
            size_words(total),
            size_words(FILE_LIMIT_BYTES)
        )
    } else {
        format!(
            "{} to send. At the current speed ({}) it takes {}. On a weak signal it can take up to {}.",
            size_words(total),
            speed_words(bps),
            duration_words(transfer_seconds(total, bps)),
            duration_words(transfer_seconds(total, 41.0))
        )
    };
    col![
        if over { text(line).caption().danger() } else { text(line).caption() }.fill_w(),
        para("Files go straight to the other station and cannot be passed on through relays, so you must be in direct range of them. Keep KAHF open and the radio connected until it finishes.", 100),
    ]
    .gap(4.0)
    .pad(10.0)
    .round(8.0)
    .outline()
    .fill_w()
    .into()
}

#[cfg(test)]
mod tests {
    use super::wrap_lines;

    #[test]
    fn wraps_on_words_within_width() {
        let lines = wrap_lines("one two three four five six", 10);
        assert_eq!(lines, vec!["one two", "three four", "five six"]);
        assert!(lines.iter().all(|l| l.chars().count() <= 10));
    }

    #[test]
    fn long_word_gets_own_line_and_empty_is_empty() {
        assert_eq!(wrap_lines("a verylongword b", 5), vec!["a", "verylongword", "b"]);
        assert!(wrap_lines("   ", 10).is_empty());
    }
}

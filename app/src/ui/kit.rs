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
    col![caption(label).dim(), control.into(), caption(hint).dim()].gap(4.0).into()
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
        c = c.push(caption(sub).dim());
    }
    c.fill_w().into()
}

/// A Test button with its answer beside it. The tests only listen or look; they never transmit.
pub fn test_button(k: &Kahf, key: &str, label: &str, run: impl Fn(&mut Kahf, &str) + Send + Sync + 'static) -> V {
    let state = k.ui.tests.get(key).cloned();
    let running = matches!(state, Some(TestState::Running));
    let key_owned = key.to_string();
    let b = button(if running { "Testing…".to_string() } else { label.to_string() })
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

/// A device list as a dropdown with "Computer default" first. Index 0 is the default.
pub fn device_pick(devices: &[String], current: &str, set: impl Fn(&mut Kahf, String) + Send + Sync + 'static) -> V {
    let mut names = vec!["Computer default".to_string()];
    names.extend(devices.iter().cloned());
    let sel = if current.is_empty() { Some(0) } else { devices.iter().position(|d| d == current).map(|i| i + 1) };
    let items = devices.to_vec();
    pick(names, sel)
        .act(move |k: &mut Kahf, i: usize| set(k, if i == 0 { String::new() } else { items[i - 1].clone() }))
        .into()
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

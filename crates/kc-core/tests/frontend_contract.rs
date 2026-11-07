//! The one contract that spans the language boundary: the slots and hideable
//! targets a plugin may name.
//!
//! `kc_core::plugin::UI_SLOTS` / `UI_HIDEABLE` are the host's allowlists;
//! `web/src/components/uiContributions.ts` is the side that actually mounts a
//! control and honours a hide. If the two drift, the failure is silent and
//! asymmetric: a plugin names a slot the host accepts and the browser never
//! mounts (a button that goes nowhere), or a hide the host allows and the client
//! ignores (a request that does nothing).
//!
//! So this test reads the frontend file and asserts every catalogued name appears
//! in it. It is a spelling check, not a type check — the point is that adding a
//! slot is never a one-sided change.

use kc_core::plugin::{UI_HIDEABLE, UI_SLOTS};
use std::path::PathBuf;

fn frontend_source() -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/src/components/uiContributions.ts");
    // A packaged crate may not carry the frontend; skip rather than fail there.
    std::fs::read_to_string(path).ok()
}

#[test]
fn every_slot_the_host_allows_is_mounted_by_the_frontend() {
    let Some(source) = frontend_source() else {
        eprintln!("skipped: web/src/components/uiContributions.ts is not in this tree");
        return;
    };
    for slot in UI_SLOTS {
        assert!(
            source.contains(&format!("'{slot}'")),
            "slot `{slot}` is in kc-core's UI_SLOTS but not in the frontend's UI_SLOT_IDS; \
             a plugin could declare a button that never mounts"
        );
    }
}

#[test]
fn every_hideable_target_is_understood_by_the_frontend() {
    let Some(source) = frontend_source() else {
        eprintln!("skipped: web/src/components/uiContributions.ts is not in this tree");
        return;
    };
    for target in UI_HIDEABLE {
        assert!(
            source.contains(&format!("'{target}'")),
            "hide target `{target}` is in kc-core's UI_HIDEABLE but not in the frontend's \
             UI_HIDE_TARGETS; the host would accept a hide the browser ignores"
        );
    }
}

#[test]
fn the_frontend_hides_nothing_the_host_would_refuse() {
    let Some(source) = frontend_source() else {
        eprintln!("skipped: web/src/components/uiContributions.ts is not in this tree");
        return;
    };
    // Targets the client *does* handle must all be host-approved, so a control
    // cannot be hidden in the app while the host still advertises it.
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("export const UI_HIDE_") else {
            continue;
        };
        let Some(value) = rest.split('=').nth(1) else {
            continue;
        };
        let Some(name) = value.split('\'').nth(1) else {
            continue;
        };
        assert!(
            UI_HIDEABLE.contains(&name),
            "the frontend defines the hide target `{name}` that kc-core's UI_HIDEABLE does not \
             allow; one side would accept what the other refuses"
        );
    }
}

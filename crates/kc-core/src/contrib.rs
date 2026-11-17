// @kc-doc: contribution-registry
// @kc-title: Extend or override harness behaviour (contribution registry)
// @kc-summary: The Add / Remove / Replace / Wrap primitive behind every extensible registry in karucode, and how precedence decides a conflict.
// @kc-tags: extensibility, registry, plugins, override, remove, precedence, hooks, tools
// @kc-applies-to: file:crates/kc-core/src/contrib.rs, hook:on_tool_call, hook:before_prompt, config:[[plugins]]
//
// # What this is
//
// Every place karucode lets a plugin change behaviour goes through
// `ContributionRegistry`. It is one data structure with four operations and one
// precedence rule, instead of a bespoke `match`, `const` slice or `Vec` per
// feature. A registry holds a set of named entries; a contribution either adds
// an entry, removes one, replaces one, or wraps one.
//
// ```rust
// use kc_core::contrib::{Contribution, ContributionRegistry, Op};
//
// let mut registry: ContributionRegistry<String> = ContributionRegistry::new("hooks");
// registry.builtin("on_run_end", "runtime".into());
//
// // A plugin adds its own.
// registry.contribute(Contribution::new("my-plugin", Op::Add, "on_my_event", "my-plugin".into(), 0));
// // A plugin removes a built-in.
// registry.contribute(Contribution::remove("guard", "on_run_end", 0));
//
// let view = registry.resolve();
// assert!(view.get("on_run_end").is_none());
// assert_eq!(view.names().collect::<Vec<_>>(), vec!["on_my_event"]);
// ```
//
// # The four operations
//
// | Op | Effect | Notes |
// | --- | --- | --- |
// | `Add` | contributes a new entry | fails if the name exists and nobody removed it |
// | `Remove` | removes an entry | removing something absent is recorded, not an error |
// | `Replace` | replaces an entry's value | the entry must exist (built-in or added) |
// | `Wrap` | runs around an existing entry | only meaningful where the consumer folds wrappers |
//
// A `Wrap` is kept separate from `Replace` on purpose: a security plugin that
// wants to *veto* rather than *override* must not have to reimplement the thing
// it wraps. The consumer decides how to fold it (for tool approval the runtime
// folds wraps inward-in, so the last wrapper sees the final answer).
//
// # Precedence
//
// Contributions are ordered by `(priority, source, sequence)`:
//
// * **priority** — higher wins, so an operator's plugin can outrank another's;
// * **source** — a stable tie-break, so two plugins with the same priority
//   resolve the same way on every run and every machine;
// * **sequence** — declaration order within one plugin, so a plugin can control
//   its own order.
//
// For `Remove` and `Replace`, the **highest-precedence claim wins**: if one
// plugin removes a built-in and another replaces it, the higher-priority one
// decides, and the loser is reported in `conflicts` rather than silently
// dropped. That is the whole point of the primitive: a conflict is *visible*.
//
// ## Set `source` from the host's registry, never from the plugin
//
// At equal priority the tie-break is `source`, so an alphabetic accident decides
// who wins: a plugin that calls itself `aaa-plugin` outranks one that calls
// itself `zzz-plugin` on the same priority, and a plugin could pick a name that
// sorts before everyone else's to win every tie.
//
// `source` is a free string, so `Contribution` cannot enforce this. The rule is
// on the caller: the host must set `source` to the **manifest name it assigned**
// to the plugin (the one it logs, keys state by and reports in diagnostics),
// never to a name taken from the plugin's own request, its manifest claim or a
// frame it wrote. Two plugins must also not be able to present the same
// `source`: the host assigns those names, so it already guarantees that. Until a
// host wires this registry that rule is unenforced, which is exactly why it is
// written down here.
//
// # What it does not do
//
// It does not sandbox, authenticate or validate values. A caller that turns a
// contribution into executable behaviour (a tool, a hook) must still run the
// host's permission and approval checks on the *result of* `resolve()`. The
// registry decides *what is in effect*; it never decides *what is allowed*.
//
// That separation is deliberate: it is why a plugin can remove a built-in hook
// without this type being able to grant it a capability it did not already have.

use std::collections::BTreeMap;

/// What a contribution does to a target name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    /// Contribute a new entry.
    Add,
    /// Remove an entry, whoever contributed it.
    Remove,
    /// Replace an entry's value, leaving its position to precedence.
    Replace,
    /// Run around an existing entry rather than instead of it.
    Wrap,
}

impl Op {
    /// The wire spelling, which is also what a manifest names.
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Add => "add",
            Op::Remove => "remove",
            Op::Replace => "replace",
            Op::Wrap => "wrap",
        }
    }
}

/// One plugin's request to change a registry.
#[derive(Debug, Clone)]
pub struct Contribution<T> {
    /// Who asked. Used as the deterministic tie-break between equal priorities,
    /// and reported in conflicts so a person can find the responsible plugin.
    pub source: String,
    /// What to do.
    pub op: Op,
    /// The name of the entry acted on. For `Add` this is the new entry's name;
    /// for the other three it names an existing one.
    pub name: String,
    /// The value: an `Add`/`Replace`/`Wrap` payload, or `None` for a `Remove`,
    /// which carries nothing. Built through [`Contribution::new`] (always
    /// `Some`) or [`Contribution::remove`] (always `None`); a `Remove` invented
    /// some other way loses any value it carries, because the operation has no
    /// use for one.
    pub value: Option<T>,
    /// Higher outranks lower. Default `0`; an operator's config can raise it.
    pub priority: i64,
}

impl<T> Contribution<T> {
    pub fn new(source: impl Into<String>, op: Op, name: impl Into<String>, value: T, priority: i64) -> Self {
        Self {
            source: source.into(),
            op,
            name: name.into(),
            value: Some(value),
            priority,
        }
    }

    /// A `Remove`, which carries no value.
    ///
    /// Separate from [`Contribution::new`] because a `Remove` has nothing to
    /// carry: the value is `Option<T>` so this needs no `T: Default`, and a trait
    /// object — the thing a tool registry removes — is not `Default` at all.
    /// There is no placeholder value to write and none to be silently ignored.
    pub fn remove(source: impl Into<String>, name: impl Into<String>, priority: i64) -> Self {
        Self {
            source: source.into(),
            op: Op::Remove,
            name: name.into(),
            value: None,
            priority,
        }
    }
}

/// Why a contribution did not take effect.
///
/// Every one of these is reported rather than swallowed: a plugin author whose
/// contribution loses must be able to read *why* in the diagnostics, which is
/// what turns "my hook doesn't fire" into a fixed bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// `Add` of a name that is already present (built-in or added, not removed).
    AlreadyPresent { name: String, source: String, held_by: String },
    /// `Replace` or `Wrap` of a name nothing provides.
    UnknownTarget { name: String, source: String },
    /// A lower-precedence `Remove`/`Replace` lost to a higher one.
    Outranked {
        name: String,
        source: String,
        lost_to: String,
    },
    /// A `Wrap` on a name that was removed by a higher-precedence claim.
    WrappedAfterRemoval { name: String, source: String },
    /// An `Add`/`Replace`/`Wrap` built without a value.
    MissingValue { name: String, source: String, op: Op },
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Conflict::AlreadyPresent { name, source, held_by } => write!(
                f,
                "`{source}` cannot add `{name}`: it is already provided by `{held_by}`"
            ),
            Conflict::UnknownTarget { name, source } => {
                write!(f, "`{source}` names `{name}`, which nothing provides")
            }
            Conflict::Outranked { name, source, lost_to } => write!(
                f,
                "`{source}`'s claim on `{name}` lost to `{lost_to}` (higher priority)"
            ),
            Conflict::WrappedAfterRemoval { name, source } => write!(
                f,
                "`{source}` cannot wrap `{name}`: it was removed"
            ),
            Conflict::MissingValue { name, source, op } => write!(
                f,
                "`{source}` cannot {} `{name}`: the contribution carries no value",
                op.as_str()
            ),
        }
    }
}

/// One entry as it will be used: its value, who provided it, and any wrappers.
#[derive(Debug, Clone)]
pub struct Resolved<T> {
    pub name: String,
    /// The effective value after the winning `Add`/`Replace`.
    pub value: T,
    /// Who provided the effective value (a plugin name, or `"builtin"`).
    pub source: String,
    /// Wrappers around `value`, ordered lowest precedence first, so a consumer
    /// folding inward-out calls the last one first.
    pub wrappers: Vec<(String, T)>,
}

/// The outcome of [`ContributionRegistry::resolve`].
#[derive(Debug, Clone)]
pub struct RegistryView<T> {
    entries: BTreeMap<String, Resolved<T>>,
    pub conflicts: Vec<Conflict>,
}

impl<T> RegistryView<T> {
    pub fn get(&self, name: &str) -> Option<&Resolved<T>> {
        self.entries.get(name)
    }

    /// Every live entry, in name order (a `BTreeMap`, so the order is stable and
    /// a snapshot of the prompt does not shuffle between calls).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Resolved<T>)> {
        self.entries.iter()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A named set of entries that plugins may add, remove, replace and wrap.
///
/// See the module documentation for the operations and the precedence rule.
#[derive(Debug, Clone)]
pub struct ContributionRegistry<T> {
    /// What this registry governs, for diagnostics ("tools", "hooks").
    kind: String,
    builtins: BTreeMap<String, T>,
    contributions: Vec<Contribution<T>>,
}

impl<T: Clone> Default for ContributionRegistry<T> {
    fn default() -> Self {
        Self {
            kind: String::new(),
            builtins: BTreeMap::new(),
            contributions: Vec::new(),
        }
    }
}

impl<T: Clone> ContributionRegistry<T> {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            builtins: BTreeMap::new(),
            contributions: Vec::new(),
        }
    }

    /// What this registry governs, for a diagnostic message.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Record an entry that exists before any plugin runs, attributed to
    /// `"builtin"`. A plugin must `Remove` or `Replace` it to change it; it
    /// cannot `Add` over it.
    pub fn builtin(&mut self, name: impl Into<String>, value: T) {
        self.builtins.insert(name.into(), value);
    }

    /// Record a plugin's request. Never fails: a losing request is reported by
    /// [`Self::resolve`] so a caller can surface every conflict at once.
    pub fn contribute(&mut self, contribution: Contribution<T>) {
        self.contributions.push(contribution);
    }

    /// Contributors in the order they are applied: highest priority first, then
    /// by `source`, then by declaration order.
    ///
    /// `source` is the second key, so at equal priority it decides. The host must
    /// set it from the manifest name it assigned, never from the plugin's own
    /// claim — see the module docs, "Set `source` from the host's registry".
    fn ordered(&self) -> Vec<&Contribution<T>> {
        let mut ordered: Vec<(usize, &Contribution<T>)> =
            self.contributions.iter().enumerate().collect();
        ordered.sort_by(|(ia, a), (ib, b)| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| ia.cmp(ib))
        });
        ordered.into_iter().map(|(_, contribution)| contribution).collect()
    }

    /// Work out what is in effect, and every request that did not take effect.
    pub fn resolve(&self) -> RegistryView<T> {
        // Start from the built-ins, attributed to `"builtin"`.
        let mut state: BTreeMap<String, Working<T>> = self
            .builtins
            .iter()
            .map(|(name, value)| (name.clone(), (value.clone(), "builtin".to_string(), Vec::new())))
            .collect();
        let mut removed: BTreeMap<String, String> = BTreeMap::new();
        // Names already *decided* by a higher-precedence `Remove` or `Replace`.
        //
        // Precedence has to decide the outcome, not just who is blamed: without
        // this, a low-priority `Remove` applied after a high-priority `Replace`
        // would still delete the entry, and the "loser" would have won. The first
        // decision on a name stands, and every later conflicting claim is
        // reported against it.
        let mut decided: BTreeMap<String, String> = BTreeMap::new();
        let mut conflicts: Vec<Conflict> = Vec::new();

        for contribution in self.ordered() {
            let name = &contribution.name;
            let source = &contribution.source;
            match contribution.op {
                Op::Add => {
                    if let Some(holder) = removed.get(name) {
                        // A higher-precedence claim removed the name; an add on a
                        // removed name is a resurrection, which is a conflict the
                        // author should see rather than a silent win.
                        conflicts.push(Conflict::Outranked {
                            name: name.clone(),
                            source: source.clone(),
                            lost_to: holder.clone(),
                        });
                    } else if let Some((_, held_by, _)) = state.get(name) {
                        conflicts.push(Conflict::AlreadyPresent {
                            name: name.clone(),
                            source: source.clone(),
                            held_by: held_by.clone(),
                        });
                    } else {
                        if let Some(value) = payload(contribution, &mut conflicts) {
                            state.insert(name.clone(), (value, source.clone(), Vec::new()));
                        }
                    }
                }
                Op::Remove => {
                    if let Some(winner) = decided.get(name) {
                        // A higher-precedence Replace already owns this name.
                        conflicts.push(Conflict::Outranked {
                            name: name.clone(),
                            source: source.clone(),
                            lost_to: winner.clone(),
                        });
                        continue;
                    }
                    match state.remove(name) {
                        Some(_) => {
                            removed.insert(name.clone(), source.clone());
                            decided.insert(name.clone(), source.clone());
                        }
                        None => {
                            if let Some(holder) = removed.get(name) {
                                conflicts.push(Conflict::Outranked {
                                    name: name.clone(),
                                    source: source.clone(),
                                    lost_to: holder.clone(),
                                });
                            } else {
                                conflicts.push(Conflict::UnknownTarget {
                                    name: name.clone(),
                                    source: source.clone(),
                                });
                            }
                        }
                    }
                }
                Op::Replace => {
                    if removed.contains_key(name) {
                        conflicts.push(Conflict::Outranked {
                            name: name.clone(),
                            source: source.clone(),
                            lost_to: removed[name].clone(),
                        });
                        continue;
                    }
                    match state.get_mut(name) {
                        Some(entry) => match payload(contribution, &mut conflicts) {
                            Some(value) => {
                                entry.0 = value;
                                entry.1 = source.clone();
                                decided.insert(name.clone(), source.clone());
                            }
                            None => continue,
                        },
                        None => conflicts.push(Conflict::UnknownTarget {
                            name: name.clone(),
                            source: source.clone(),
                        }),
                    }
                }
                Op::Wrap => {
                    if removed.contains_key(name) {
                        conflicts.push(Conflict::WrappedAfterRemoval {
                            name: name.clone(),
                            source: source.clone(),
                        });
                        continue;
                    }
                    match state.get_mut(name) {
                        // Appended in apply order, which is precedence order, so
                        // the highest-precedence wrapper is last.
                        Some(entry) => match payload(contribution, &mut conflicts) {
                            Some(value) => entry.2.push((source.clone(), value)),
                            None => continue,
                        },
                        None => conflicts.push(Conflict::UnknownTarget {
                            name: name.clone(),
                            source: source.clone(),
                        }),
                    }
                }
            }
        }

        let entries = state
            .into_iter()
            .map(|(name, (value, source, wrappers))| {
                (
                    name.clone(),
                    Resolved {
                        name,
                        value,
                        source,
                        wrappers,
                    },
                )
            })
            .collect();
        RegistryView { entries, conflicts }
    }
}

/// One entry while `resolve` is applying contributions: its current value, who
/// provided it, and the wrappers recorded so far (in apply order).
type Working<T> = (T, String, Vec<(String, T)>);

/// The value a contribution carries, or a reported conflict when it carries
/// none.
///
/// Only an `Add`, `Replace` or `Wrap` reaches here and every one of those needs
/// a value; [`Contribution::new`] always gives it one and [`Contribution::remove`]
/// only ever builds a `Remove`. A hand-built `Op::Add` with `value: None` would
/// otherwise have to invent a payload, so it is refused and reported like any
/// other contribution that cannot take effect.
fn payload<T: Clone>(contribution: &Contribution<T>, conflicts: &mut Vec<Conflict>) -> Option<T> {
    if contribution.value.is_some() {
        return contribution.value.clone();
    }
    conflicts.push(Conflict::MissingValue {
        name: contribution.name.clone(),
        source: contribution.source.clone(),
        op: contribution.op,
    });
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> ContributionRegistry<&'static str> {
        let mut registry = ContributionRegistry::new("hooks");
        registry.builtin("on_run_end", "runtime");
        registry.builtin("on_tool_call", "runtime");
        registry
    }

    #[test]
    fn builtins_are_present_until_something_changes_them() {
        let view = reg().resolve();
        assert_eq!(view.len(), 2);
        assert_eq!(view.get("on_run_end").unwrap().source, "builtin");
        assert!(view.conflicts.is_empty());
    }

    #[test]
    fn a_plugin_can_add_a_new_entry() {
        let mut registry = reg();
        registry.contribute(Contribution::new("mine", Op::Add, "on_my_event", "mine", 0));
        let view = registry.resolve();
        assert_eq!(view.get("on_my_event").unwrap().value, "mine");
        assert_eq!(view.len(), 3);
    }

    #[test]
    fn a_plugin_can_remove_a_builtin_entry() {
        let mut registry = reg();
        registry.contribute(Contribution::remove("guard", "on_run_end", 0));
        let view = registry.resolve();
        assert!(view.get("on_run_end").is_none(), "removal must take effect");
        assert!(view.get("on_tool_call").is_some(), "and only that one");
    }

    /// `remove` must not need `T: Default` and must not carry a placeholder. A
    /// trait object is the real `T` here and it is not `Default`, which is why
    /// the value is an `Option`.
    #[test]
    fn remove_carries_no_value_and_needs_no_default() {
        use std::sync::Arc;

        trait Marker: Send + Sync {}
        struct A;
        struct B(#[allow(dead_code)] u8);
        impl Marker for A {}
        impl Marker for B {}

        let contribution = Contribution::<Arc<dyn Marker>>::remove("guard", "run_command", 0);
        assert_eq!(contribution.op, Op::Remove);
        assert!(contribution.value.is_none(), "a Remove never carries a payload");

        // The value a Remove used to be built around does not exist, so the
        // registry has to work with types that cannot make one.
        let mut registry: ContributionRegistry<Arc<dyn Marker>> =
            ContributionRegistry::new("tools");
        registry.builtin("run_command", Arc::new(A));
        registry.contribute(Contribution::remove("guard", "run_command", 0));
        registry.contribute(Contribution::new("mine", Op::Add, "my_tool", Arc::new(B(1)), 0));
        let view = registry.resolve();
        assert!(view.get("run_command").is_none());
        assert!(view.get("my_tool").is_some());
        assert!(view.conflicts.is_empty(), "{:?}", view.conflicts);

        // And `new` always supplies one.
        assert!(Contribution::new("mine", Op::Add, "x", Arc::new(A), 0)
            .value
            .is_some());
    }

    /// A contribution that needs a value and has none is reported, not silently
    /// applied with an invented payload.
    #[test]
    fn a_valueless_add_or_replace_is_reported() {
        let mut registry = reg();
        registry.contribute(Contribution {
            source: "handbuilt".into(),
            op: Op::Add,
            name: "on_extra".into(),
            value: None,
            priority: 0,
        });
        registry.contribute(Contribution {
            source: "handbuilt".into(),
            op: Op::Replace,
            name: "on_run_end".into(),
            value: None,
            priority: 0,
        });
        let view = registry.resolve();
        assert_eq!(view.get("on_run_end").unwrap().value, "runtime", "unchanged");
        assert!(view.get("on_extra").is_none());
        assert_eq!(
            view.conflicts
                .iter()
                .filter(|c| matches!(c, Conflict::MissingValue { .. }))
                .count(),
            2,
            "{:?}",
            view.conflicts
        );
        assert!(view.conflicts[1].to_string().contains("carries no value"));
    }

    #[test]
    fn a_plugin_can_replace_a_builtin_value() {
        let mut registry = reg();
        registry.contribute(Contribution::new("mine", Op::Replace, "on_run_end", "mine", 0));
        let view = registry.resolve();
        let entry = view.get("on_run_end").unwrap();
        assert_eq!(entry.value, "mine");
        assert_eq!(entry.source, "mine");
    }

    #[test]
    fn a_wrap_keeps_the_original_and_records_the_wrapper() {
        let mut registry = reg();
        registry.contribute(Contribution::new("veto", Op::Wrap, "on_tool_call", "veto", 0));
        let view = registry.resolve();
        let entry = view.get("on_tool_call").unwrap();
        assert_eq!(entry.value, "runtime", "a wrap does not replace");
        assert_eq!(entry.wrappers.len(), 1);
        assert_eq!(entry.wrappers[0].0, "veto");
    }

    #[test]
    fn adding_over_an_existing_name_is_a_reported_conflict() {
        let mut registry = reg();
        registry.contribute(Contribution::new("mine", Op::Add, "on_run_end", "mine", 0));
        let view = registry.resolve();
        assert_eq!(view.get("on_run_end").unwrap().value, "runtime");
        assert!(matches!(
            view.conflicts[0],
            Conflict::AlreadyPresent { .. }
        ));
        assert!(view.conflicts[0].to_string().contains("on_run_end"));
    }

    #[test]
    fn higher_priority_wins_and_the_loser_is_reported() {
        let mut registry = reg();
        // Low priority removes; high priority replaces.
        registry.contribute(Contribution::remove("low", "on_run_end", 0));
        registry.contribute(Contribution::new("high", Op::Replace, "on_run_end", "high", 10));
        let view = registry.resolve();
        assert_eq!(
            view.get("on_run_end").unwrap().value,
            "high",
            "the higher priority claim decides"
        );
        assert!(
            view.conflicts.iter().any(|c| matches!(c, Conflict::Outranked { source, .. } if source == "low")),
            "and the loser is reported: {:?}",
            view.conflicts
        );
    }

    #[test]
    fn equal_priority_resolves_by_source_name_so_runs_are_deterministic() {
        let mut registry = reg();
        registry.contribute(Contribution::new("zeta", Op::Replace, "on_run_end", "zeta", 0));
        registry.contribute(Contribution::remove("alpha", "on_run_end", 0));
        // "alpha" < "zeta", so alpha applies first and zeta's replace then fails
        // (the target is gone) — which must be reported, not silently ignored.
        let view = registry.resolve();
        assert!(view.get("on_run_end").is_none(), "alpha's removal applied first");
        assert!(view
            .conflicts
            .iter()
            .any(|c| matches!(c, Conflict::Outranked { source, .. } if source == "zeta")));
    }

    #[test]
    fn wrapping_a_removed_entry_is_reported() {
        let mut registry = reg();
        registry.contribute(Contribution::remove("a", "on_tool_call", 5));
        registry.contribute(Contribution::new("b", Op::Wrap, "on_tool_call", "b", 0));
        let view = registry.resolve();
        assert!(view.get("on_tool_call").is_none());
        assert!(matches!(
            view.conflicts[0],
            Conflict::WrappedAfterRemoval { .. }
        ));
    }

    #[test]
    fn resolve_is_deterministic_regardless_of_declaration_order() {
        let mut first = reg();
        first.contribute(Contribution::remove("a", "on_run_end", 0));
        first.contribute(Contribution::new("b", Op::Replace, "on_run_end", "b", 0));

        let mut second = reg();
        second.contribute(Contribution::new("b", Op::Replace, "on_run_end", "b", 0));
        second.contribute(Contribution::remove("a", "on_run_end", 0));

        let one = first.resolve();
        let two = second.resolve();
        assert_eq!(one.get("on_run_end").is_some(), two.get("on_run_end").is_some());
        assert_eq!(one.conflicts.len(), two.conflicts.len());
    }

    /// Precedence is a total order, so two orderings of the same contributions
    /// resolve to the same values *and* the same blame. Anything less means a
    /// conflict's outcome depends on the order the host happened to collect
    /// plugins in.
    #[test]
    fn precedence_is_identical_whichever_order_contributions_arrive_in() {
        let build = |order: [usize; 4]| {
            let all = [
                Contribution::remove("alpha", "on_run_end", 0),
                Contribution::new("zeta", Op::Replace, "on_run_end", "zeta", 0),
                Contribution::new("beta", Op::Wrap, "on_tool_call", "beta", 1),
                Contribution::new("gamma", Op::Wrap, "on_tool_call", "gamma", 1),
            ];
            let mut registry = reg();
            for index in order {
                registry.contribute(all[index].clone());
            }
            registry.resolve()
        };
        let one = build([0, 1, 2, 3]);
        let two = build([3, 2, 1, 0]);

        let end = one.get("on_run_end");
        assert_eq!(end.is_none(), two.get("on_run_end").is_none());
        // Equal priority, so `alpha` < `zeta`: alpha removes first, zeta's
        // replace then loses the name — the same whichever order they arrived.
        assert!(end.is_none(), "equal priority resolves by source, not arrival");
        assert_eq!(one.conflicts, two.conflicts);

        // Wrappers keep precedence order (lowest first) regardless of arrival.
        let wrappers = |view: &RegistryView<&'static str>| {
            view.get("on_tool_call")
                .unwrap()
                .wrappers
                .iter()
                .map(|(source, _)| source.clone())
                .collect::<Vec<String>>()
        };
        assert_eq!(wrappers(&one), vec!["beta", "gamma"], "source breaks the tie");
        assert_eq!(wrappers(&two), wrappers(&one));
    }

    /// The tie-break at equal priority is `source` alone, which is exactly why
    /// the host must assign it: the alphabet decides otherwise.
    #[test]
    fn equal_priority_is_decided_by_the_source_string() {
        let claim = |sources: (&'static str, &'static str)| {
            let mut registry = reg();
            registry.contribute(Contribution::remove(sources.0, "on_run_end", 0));
            registry.contribute(Contribution::new(sources.1, Op::Replace, "on_run_end", "kept", 0));
            let view = registry.resolve();
            (view.get("on_run_end").is_none(), view.conflicts.len())
        };
        // Whoever sorts first applies first, and its decision stands.
        assert_eq!(claim(("aaa", "zzz")), (true, 1), "aaa's remove wins the tie");
        assert_eq!(claim(("zzz", "aaa")), (false, 1), "aaa's replace wins the tie");
    }

    #[test]
    fn entry_order_is_stable_for_a_prompt_snapshot() {
        let mut registry = ContributionRegistry::new("tools");
        registry.builtin("zeta", "z");
        registry.builtin("alpha", "a");
        let view = registry.resolve();
        assert_eq!(
            view.names().collect::<Vec<_>>(),
            vec!["alpha", "zeta"],
            "BTreeMap order, so a prompt does not shuffle between calls"
        );
    }
}

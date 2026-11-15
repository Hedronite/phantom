//! The hand-built bucket cases: a store written by hand and scored
//! through the report. Moved whole from the scorer's test file; no
//! logic, name or output changed.
use super::*;

pub fn each_d4_bucket_classifies_its_hand_built_refs() {
    use scip::score::w12_bucket;

    let reference =
        |kind: &str, text: &str, receiver: Option<&str>, hint: Option<&str>| ScoreCallRef {
            kind: kind.to_owned(),
            text: text.to_owned(),
            receiver: receiver.map(|text| text.to_owned()),
            receiver_type: hint.map(|hint| hint.to_owned()),
            scope: String::new(),
            placed: false,
        };
    let one_ref = |kind: &str, receiver: Option<&str>, hint: Option<&str>| {
        vec![reference(kind, "f", receiver, hint)]
    };

    assert_eq!(
        w12_bucket(&one_ref("macro_arg", None, None), "f", false),
        "macro_arg path shape"
    );
    assert_eq!(
        w12_bucket(&one_ref("macro_arg", Some("x"), None), "f", false),
        "macro_arg method shape"
    );
    assert_eq!(
        w12_bucket(&one_ref("macro", None, None), "f", false),
        "macro invocation"
    );
    // A macro_arg ref outranks the invocation that carries it.
    let both = vec![
        reference("macro_arg", "f", Some("x"), None),
        reference("macro", "f", None, None),
    ];
    assert_eq!(w12_bucket(&both, "f", false), "macro_arg method shape");
    let both = vec![
        reference("macro_arg", "f", None, None),
        reference("macro", "f", None, None),
    ];
    assert_eq!(w12_bucket(&both, "f", false), "macro_arg path shape");

    assert_eq!(
        w12_bucket(&one_ref("call", None, None), "f", false),
        "unresolved path call"
    );
    assert_eq!(
        w12_bucket(&one_ref("method_call", Some("x"), None), "f", true),
        "trait-method callee"
    );
    for (hint, bucket) in [
        ("type:T", "identifier with hint type"),
        ("ctor:T::f", "identifier with hint ctor"),
        ("ctor?:T::f", "identifier with hint ctor?"),
        ("ret:f", "identifier with hint ret"),
        ("ret?:f", "identifier with hint ret?"),
    ] {
        assert_eq!(
            w12_bucket(&one_ref("method_call", Some("x"), Some(hint)), "f", false),
            bucket,
            "{hint}"
        );
    }
    assert_eq!(
        w12_bucket(
            &one_ref("method_call", Some("x"), Some("bogus:z")),
            "f",
            false
        ),
        "other"
    );
    assert_eq!(
        w12_bucket(&one_ref("method_call", Some("self"), None), "f", false),
        "identifier with no hint"
    );
    assert_eq!(
        w12_bucket(&one_ref("method_call", Some("self.f"), None), "f", false),
        "self.field"
    );
    for receiver in ["make()", "a.b", "self.f.g()"] {
        assert_eq!(
            w12_bucket(&one_ref("method_call", Some(receiver), None), "f", false),
            "call or chained receiver",
            "{receiver}"
        );
    }
    for receiver in ["v[0]", "<T as U>"] {
        assert_eq!(
            w12_bucket(&one_ref("method_call", Some(receiver), None), "f", false),
            "other",
            "{receiver}"
        );
    }
    let empty: Vec<ScoreCallRef> = Vec::new();
    assert_eq!(w12_bucket(&empty, "f", false), "other");

    // The precedence with several same-named refs in one caller: the
    // hint kind, then the unhinted identifier, self.field, the call or
    // chained receiver, and the catch-all.
    let mixed = vec![
        reference("method_call", "f", Some("make()"), None),
        reference("method_call", "f", Some("self.f"), None),
        reference("method_call", "f", Some("n2"), None),
        reference("method_call", "f", Some("x"), Some("type:T")),
    ];
    assert_eq!(w12_bucket(&mixed, "f", false), "identifier with hint type");
    let mixed = vec![
        reference("method_call", "f", Some("make()"), None),
        reference("method_call", "f", Some("self.f"), None),
        reference("method_call", "f", Some("n2"), None),
    ];
    assert_eq!(w12_bucket(&mixed, "f", false), "identifier with no hint");
    let mixed = vec![
        reference("method_call", "f", Some("make()"), None),
        reference("method_call", "f", Some("self.f"), None),
    ];
    assert_eq!(w12_bucket(&mixed, "f", false), "self.field");
}

pub fn each_w13_bucket_classifies_its_hand_built_refs() {
    use scip::foreign::{Binding, MissContext};
    use scip::score::MissInput;
    use scip::score::miss_bucket;

    let reference = |kind: &str, text: &str| ScoreCallRef {
        kind: kind.to_owned(),
        text: text.to_owned(),
        receiver: None,
        receiver_type: None,
        scope: String::new(),
        placed: false,
    };
    let empty = || MissContext::default();

    // The W12 ladder reads through an empty context unchanged.
    assert_eq!(
        miss_bucket(
            &empty(),
            MissInput {
                refs: &[reference("call", "f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 1,
                callee_id: 2,
            },
        ),
        ("unresolved path call", "unresolved path call")
    );

    // shared module file: the caller or the callee sits in a file more
    // than one crate target claims, whatever the family.
    let shared = MissContext {
        shared: [7].into(),
        ..MissContext::default()
    };
    assert_eq!(
        miss_bucket(
            &shared,
            MissInput {
                refs: &[reference("call", "f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 7,
                callee_file: 1,
                callee_id: 2,
            },
        ),
        ("shared module file", "unresolved path call")
    );
    assert_eq!(
        miss_bucket(
            &shared,
            MissInput {
                refs: &[reference("method_call", "f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 7,
                callee_id: 2,
            },
        ),
        ("shared module file", "other")
    );

    // glob import in scope: the callee is defined in a module row the
    // caller's scope's glob use reaches.
    let glob = MissContext {
        glob: [((1, String::new()), [5].into())].into(),
        symbol_module: [(9, 5)].into(),
        ..MissContext::default()
    };
    assert_eq!(
        miss_bucket(
            &glob,
            MissInput {
                refs: &[reference("call", "f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 1,
                callee_id: 9,
            },
        ),
        ("glob import in scope", "unresolved path call")
    );
    // A callee defined elsewhere stays with the W12 bucket.
    assert_eq!(
        miss_bucket(
            &glob,
            MissInput {
                refs: &[reference("call", "f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 1,
                callee_id: 10,
            },
        ),
        ("unresolved path call", "unresolved path call")
    );

    // re-exported path: the first segment binds an item whose home
    // module differs from the module the binding's path names.
    let reexport = MissContext {
        uses: [(
            (1, String::new()),
            vec![Binding {
                bound: "g".to_owned(),
                prefix: "crate".to_owned(),
                home: "crate::inner".to_owned(),
            }],
        )]
        .into(),
        ..MissContext::default()
    };
    assert_eq!(
        miss_bucket(
            &reexport,
            MissInput {
                refs: &[reference("call", "g::f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 1,
                callee_id: 2,
            },
        ),
        ("re-exported path", "unresolved path call")
    );
    assert_eq!(
        miss_bucket(
            &reexport,
            MissInput {
                refs: &[reference("call", "crate::f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 1,
                callee_id: 2,
            },
        ),
        ("unresolved path call", "unresolved path call")
    );

    // The shared-file mark outranks the glob reach, and a miss still
    // lands in exactly one bucket.
    let both = MissContext {
        shared: [1].into(),
        glob: [((1, String::new()), [5].into())].into(),
        symbol_module: [(9, 5)].into(),
        ..MissContext::default()
    };
    assert_eq!(
        miss_bucket(
            &both,
            MissInput {
                refs: &[reference("call", "f")],
                callee_name: "f",
                is_trait: false,
                caller_file: 1,
                callee_file: 1,
                callee_id: 9,
            },
        ),
        ("shared module file", "unresolved path call")
    );
}

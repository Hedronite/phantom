//! JSON envelope for Phantom.
//!
//! Success carries `ok` and `data`. Failure carries `error.kind`,
//! `error.message`, and `error.exit`. [`ErrorKind`] names canon exits
//! 0 through 6. JSON fields this crate does not define are ignored.

use serde::{Deserialize, Serialize};

/// Canon exits 0 through 6, named.
///
/// These names are not a CLI. The process exit is [`ErrorKind::exit_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Exit 0. The call succeeded.
    Ok,
    /// Exit 1. Usage, or an internal failure.
    Usage,
    /// Exit 2. This repo has no index.
    NoIndex,
    /// Exit 3. A path, repo, or symbol was not found.
    NotFound,
    /// Exit 4. Another process holds the writer lock.
    LockHeld,
    /// Exit 5. The stored schema is newer than this binary.
    SchemaNewer,
    /// Exit 6. The target matched more than one candidate.
    Ambiguous,
}

impl ErrorKind {
    /// Process exit for this kind. Canon numbers 0 through 6.
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Usage => 1,
            Self::NoIndex => 2,
            Self::NotFound => 3,
            Self::LockHeld => 4,
            Self::SchemaNewer => 5,
            Self::Ambiguous => 6,
        }
    }
}

/// `error` object: kind, message, and exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeError {
    pub kind: ErrorKind,
    pub message: String,
    pub exit: i32,
}

impl EnvelopeError {
    /// Builds an error whose `exit` is the canon code for `kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            exit: kind.exit_code(),
        }
    }
}

/// `{ ok, data, error }`. Extra JSON fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<EnvelopeError>,
}

impl<T> Envelope<T> {
    /// A success value. `error` is absent.
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    /// A failure value. `data` is absent. `exit` follows `kind`.
    pub fn failure(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(EnvelopeError::new(kind, message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn success_and_error_round_trip() {
        let success = Envelope::success(String::from("row"));
        let error = Envelope::<String>::failure(ErrorKind::NotFound, "missing");

        assert_eq!(round_trip(&success), success);
        assert_eq!(round_trip(&error), error);

        let success_json = serde_json::to_value(&success).expect("success json");
        assert_eq!(success_json["ok"], true);
        assert_eq!(success_json["data"], "row");
        assert!(success_json["error"].is_null());

        let error_json = serde_json::to_value(&error).expect("error json");
        assert_eq!(error_json["ok"], false);
        assert!(error_json["data"].is_null());
        assert_eq!(error_json["error"]["kind"], "not_found");
        assert_eq!(error_json["error"]["message"], "missing");
        assert_eq!(error_json["error"]["exit"], 3);
    }

    #[test]
    fn kinds_name_exits_zero_through_six() {
        let named = [
            (ErrorKind::Ok, 0, "ok"),
            (ErrorKind::Usage, 1, "usage"),
            (ErrorKind::NoIndex, 2, "no_index"),
            (ErrorKind::NotFound, 3, "not_found"),
            (ErrorKind::LockHeld, 4, "lock_held"),
            (ErrorKind::SchemaNewer, 5, "schema_newer"),
            (ErrorKind::Ambiguous, 6, "ambiguous"),
        ];
        assert_eq!(named.len(), 7);
        for (kind, exit, name) in named {
            assert_eq!(kind.exit_code(), exit);
            let json = serde_json::to_string(&kind).expect("kind json");
            assert_eq!(json, format!("\"{name}\""));
        }
    }

    #[test]
    fn unknown_json_fields_are_ignored() {
        let raw = r#"{
            "ok": false,
            "data": null,
            "error": {
                "kind": "ambiguous",
                "message": "two hits",
                "exit": 6,
                "hint": "pick one"
            },
            "meta": { "latency_ms": 4 },
            "trace": []
        }"#;
        let parsed: Envelope<String> = serde_json::from_str(raw).expect("parse");
        let expected = Envelope {
            ok: false,
            data: None,
            error: Some(EnvelopeError::new(ErrorKind::Ambiguous, "two hits")),
        };
        assert_eq!(parsed, expected);
    }
}

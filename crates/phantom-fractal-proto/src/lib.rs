#![forbid(unsafe_code)]

//! JSON envelope for Phantom.
//!
//! Success carries `ok`, `data`, and `schema`. Failure carries `error.kind`,
//! `error.message`, and `error.exit`, plus `schema`. [`ErrorKind`] names
//! canon exits 0 through 6. [`SCHEMA_VERSION`] is the schema number a Lapis
//! reader checks on the `schema` field. JSON fields this crate does not
//! define are ignored.
//!
//! [`EnvelopeError::message`] must not contain secrets or absolute paths.
//! Callers sanitize that text before they build an error.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

/// Schema number written by [`Envelope::success`] and [`Envelope::failure`].
///
/// A Lapis reader compares the JSON field `schema` with this constant
/// before it trusts `data`.
pub const SCHEMA_VERSION: u32 = 1;

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

/// One failure. `exit` is [`ErrorKind::exit_code`] for `kind`, not a
/// separate value a caller can set.
///
/// `message` is caller-supplied. Callers must sanitize it. The string
/// must not contain secrets or absolute paths. This crate stores the
/// text unchanged and does not redact it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeError {
    pub kind: ErrorKind,
    /// Failure text. Callers sanitize this before [`EnvelopeError::new`].
    /// It must not contain secrets or absolute paths.
    pub message: String,
}

impl EnvelopeError {
    /// Builds an error. `exit` is `kind.exit_code()` and is not stored apart
    /// from `kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Canon exit for [`Self::kind`].
    pub const fn exit(&self) -> i32 {
        self.kind.exit_code()
    }
}

impl Serialize for EnvelopeError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("EnvelopeError", 3)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("message", &self.message)?;
        state.serialize_field("exit", &self.exit())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for EnvelopeError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            kind: ErrorKind,
            message: String,
            exit: i32,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.exit != raw.kind.exit_code() {
            return Err(de::Error::custom("exit does not match kind"));
        }
        Ok(Self {
            kind: raw.kind,
            message: raw.message,
        })
    }
}

/// `{ ok, data, error, schema }`.
///
/// Success has no error. Failure has an error. A value with `ok: true` and
/// an error set cannot be built, and neither can a failure whose `exit`
/// disagrees with its kind. Extra JSON fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Envelope<T> {
    /// `ok` is true and `error` is absent.
    Success { data: Option<T>, schema: u32 },
    /// `ok` is false and `error` is present. `data` may still hold
    /// candidates when the kind is [`ErrorKind::Ambiguous`].
    Failure {
        error: EnvelopeError,
        data: Option<T>,
        schema: u32,
    },
}

impl<T> Envelope<T> {
    /// A success value. `schema` is [`SCHEMA_VERSION`].
    pub fn success(data: T) -> Self {
        Self::Success {
            data: Some(data),
            schema: SCHEMA_VERSION,
        }
    }

    /// A failure value. `data` is absent. `schema` is [`SCHEMA_VERSION`].
    /// `exit` follows `kind`.
    pub fn failure(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Failure {
            error: EnvelopeError::new(kind, message),
            data: None,
            schema: SCHEMA_VERSION,
        }
    }

    /// A failure that still carries `data`, such as the candidates of an
    /// [`ErrorKind::Ambiguous`] lookup. `schema` is [`SCHEMA_VERSION`].
    /// `exit` follows `kind`.
    pub fn failure_with_data(kind: ErrorKind, message: impl Into<String>, data: T) -> Self {
        Self::Failure {
            error: EnvelopeError::new(kind, message),
            data: Some(data),
            schema: SCHEMA_VERSION,
        }
    }

    /// True when this value is [`Self::Success`].
    pub const fn ok(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Payload, when one was supplied.
    pub const fn data(&self) -> Option<&T> {
        match self {
            Self::Success { data, .. } | Self::Failure { data, .. } => data.as_ref(),
        }
    }

    /// Failure body. Absent on success.
    pub const fn error(&self) -> Option<&EnvelopeError> {
        match self {
            Self::Success { .. } => None,
            Self::Failure { error, .. } => Some(error),
        }
    }

    /// Schema number from the wire, or [`SCHEMA_VERSION`] for values this
    /// crate built.
    pub const fn schema(&self) -> u32 {
        match self {
            Self::Success { schema, .. } | Self::Failure { schema, .. } => *schema,
        }
    }

    fn from_parts(
        ok: bool,
        data: Option<T>,
        error: Option<EnvelopeError>,
        schema: u32,
    ) -> Result<Self, &'static str> {
        match (ok, error) {
            (true, None) => Ok(Self::Success { data, schema }),
            (false, Some(error)) => Ok(Self::Failure {
                error,
                data,
                schema,
            }),
            (true, Some(_)) => Err("ok is true while error is set"),
            (false, None) => Err("ok is false while error is absent"),
        }
    }
}

impl<T> Serialize for Envelope<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Envelope", 4)?;
        state.serialize_field("ok", &self.ok())?;
        state.serialize_field("data", &self.data())?;
        state.serialize_field("error", &self.error())?;
        state.serialize_field("schema", &self.schema())?;
        state.end()
    }
}

impl<'de, T> Deserialize<'de> for Envelope<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw<T> {
            ok: bool,
            data: Option<T>,
            error: Option<EnvelopeError>,
            schema: u32,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::from_parts(raw.ok, raw.data, raw.error, raw.schema).map_err(de::Error::custom)
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
        assert_eq!(success.schema(), SCHEMA_VERSION);
        assert_eq!(error.schema(), SCHEMA_VERSION);

        let success_json = serde_json::to_value(&success).expect("success json");
        assert_eq!(success_json["ok"], true);
        assert_eq!(success_json["data"], "row");
        assert!(success_json["error"].is_null());
        assert_eq!(success_json["schema"], SCHEMA_VERSION);

        let error_json = serde_json::to_value(&error).expect("error json");
        assert_eq!(error_json["ok"], false);
        assert!(error_json["data"].is_null());
        assert_eq!(error_json["error"]["kind"], "not_found");
        assert_eq!(error_json["error"]["message"], "missing");
        assert_eq!(error_json["error"]["exit"], 3);
        assert_eq!(error_json["schema"], SCHEMA_VERSION);
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
            assert_eq!(EnvelopeError::new(kind, "x").exit(), exit);
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
            "schema": 1,
            "meta": { "latency_ms": 4 },
            "trace": []
        }"#;
        let parsed: Envelope<String> = serde_json::from_str(raw).expect("parse");
        let expected = Envelope::<String>::failure(ErrorKind::Ambiguous, "two hits");
        assert_eq!(parsed, expected);
    }

    #[test]
    fn contradictory_envelopes_do_not_parse() {
        let samples = [
            r#"{"ok":true,"data":"row","error":{"kind":"usage","message":"no","exit":1},"schema":1}"#,
            r#"{"ok":false,"data":null,"error":null,"schema":1}"#,
            r#"{"ok":false,"data":null,"error":{"kind":"not_found","message":"missing","exit":1},"schema":1}"#,
        ];
        for raw in samples {
            let parsed = serde_json::from_str::<Envelope<String>>(raw);
            assert!(parsed.is_err(), "{raw}");
        }
    }

    #[test]
    fn schema_field_is_what_a_reader_checks() {
        assert_eq!(SCHEMA_VERSION, 1);
        let missing = r#"{"ok":true,"data":"row","error":null}"#;
        assert!(serde_json::from_str::<Envelope<String>>(missing).is_err());

        let other = r#"{"ok":true,"data":"row","error":null,"schema":2}"#;
        let parsed: Envelope<String> = serde_json::from_str(other).expect("parse");
        assert_eq!(parsed.schema(), 2);
        assert_ne!(parsed.schema(), SCHEMA_VERSION);
    }

    #[test]
    fn ambiguous_failure_keeps_candidate_data() {
        let raw = r#"{"ok":false,"data":["a","b"],"error":{"kind":"ambiguous","message":"two hits","exit":6},"schema":1}"#;
        let parsed: Envelope<Vec<String>> = serde_json::from_str(raw).expect("parse");
        assert!(!parsed.ok());
        let items = parsed.data().expect("candidates");
        assert_eq!(items.as_slice(), ["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn failure_with_data_serializes_both() {
        let built = Envelope::failure_with_data(
            ErrorKind::Ambiguous,
            "two hits",
            vec!["a".to_string(), "b".to_string()],
        );
        assert!(!built.ok());
        assert_eq!(built.schema(), SCHEMA_VERSION);
        assert_eq!(round_trip(&built), built);
        let json = serde_json::to_value(&built).expect("json");
        assert_eq!(json["ok"], false);
        assert_eq!(json["data"], serde_json::json!(["a", "b"]));
        assert_eq!(json["error"]["kind"], "ambiguous");
        assert_eq!(json["error"]["exit"], 6);
        assert_eq!(json["schema"], 1);
    }
}

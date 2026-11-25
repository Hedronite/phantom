//! A limit above the cap (SPEC-w16 G1b) is used as the cap, and the reply
//! says so: `meta.limit` holds `{asked, used}` and `data.truncated` is
//! true. A limit of 0 is still usage.

use phantom_fractal_proto::{ErrorKind, LimitNote};

use super::Failure;

/// The smallest limit, and the most a command lists for one.
pub(crate) const LIMIT_MIN: u64 = 1;
pub(crate) const LIMIT_CAP: u64 = 100;

/// A limit as asked for, at least [`LIMIT_MIN`].
#[derive(Clone, Copy)]
pub(crate) struct Limit {
    asked: u64,
}

impl Limit {
    pub(crate) fn new(asked: u64) -> Result<Self, Failure> {
        if asked < LIMIT_MIN {
            return Err(Failure::new(
                ErrorKind::Usage,
                format!("limit must be at least {LIMIT_MIN}"),
            ));
        }
        Ok(Self { asked })
    }

    /// The limit the command works with, at most [`LIMIT_CAP`].
    pub(crate) fn used(self) -> u64 {
        self.asked.min(LIMIT_CAP)
    }

    /// `data.truncated`: true when `cut` entries were left out, and always
    /// when the limit itself was cut to the cap.
    pub(crate) fn truncated(self, cut: bool) -> bool {
        cut || self.note().is_some()
    }

    /// What `meta.limit` says: nothing unless the limit was cut to the cap.
    pub(crate) fn note(self) -> Option<LimitNote> {
        (self.asked > LIMIT_CAP).then(|| LimitNote {
            asked: self.asked,
            used: self.used(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LIMIT_CAP, Limit};

    #[test]
    fn only_a_limit_above_the_cap_is_noted() {
        assert!(Limit::new(0).is_err());
        for asked in [1, 20, LIMIT_CAP] {
            let limit = Limit::new(asked).ok().expect("in range");
            assert_eq!((limit.used(), limit.note()), (asked, None));
            assert_eq!(
                (limit.truncated(false), limit.truncated(true)),
                (false, true)
            );
        }
        for asked in [LIMIT_CAP + 1, 200, u64::MAX] {
            let limit = Limit::new(asked).ok().expect("clamped");
            let note = limit.note().expect("noted");
            assert_eq!(limit.used(), LIMIT_CAP);
            assert!(limit.truncated(false) && limit.truncated(true));
            assert_eq!((note.asked, note.used), (asked, LIMIT_CAP));
        }
    }
}

use serde::{Deserialize, Serialize};

use crate::{NonEmptyString, Reason, Slug};

/// Metadata keeps its ordinary value when known, or an explicit gap reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Observed<T> {
    Unavailable(UnavailableMetadata),
    Available(T),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnavailableMetadata {
    availability: Unavailable,
    reason: Reason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Unavailable {
    Unavailable,
}

impl<T> Observed<T> {
    #[must_use]
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Available(value) => Some(value),
            Self::Unavailable(_) => None,
        }
    }

    #[must_use]
    pub const fn unavailable(reason: Reason) -> Self {
        Self::Unavailable(UnavailableMetadata {
            availability: Unavailable::Unavailable,
            reason,
        })
    }

    pub(crate) fn gap(code: &str, message: &str) -> Self {
        Self::unavailable(Reason::new(
            Slug::new(code).expect("static metadata reason code"),
            NonEmptyString::new(message).expect("static metadata reason message"),
        ))
    }
}

impl<T> From<T> for Observed<T> {
    fn from(value: T) -> Self {
        Self::Available(value)
    }
}

#[cfg(test)]
mod tests {
    use super::Observed;
    use std::num::NonZeroU64;

    #[test]
    fn missing_metadata_is_not_zero_null_or_an_invented_value() {
        let gap = Observed::<NonZeroU64>::gap("not_exposed", "Host API did not expose this field.");
        let json = serde_json::to_string(&gap).unwrap();
        assert_eq!(
            serde_json::from_str::<Observed<NonZeroU64>>(&json).unwrap(),
            gap
        );
        for invalid in ["0", "null", r#"{"availability":"unavailable"}"#] {
            assert!(serde_json::from_str::<Observed<NonZeroU64>>(invalid).is_err());
        }
    }
}

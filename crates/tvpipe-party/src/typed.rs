//! Game-defined message types with [`deser`] (feature `deser`,
//! docs/multiscreen-design.md §10.1): derive `Serialize`/`Deserialize`
//! on the game's types and convert from and to the party's [`Value`]s.
//!
//! The conversion goes through JSON text (`deser-json`): the party parses
//! what phones send once, with its own limits, and messages are small.

use crate::party::{Action, PartyError, State};
use crate::value::Value;

/// A game type as a [`Value`].
pub fn to_value<T: deser::Serialize + ?Sized>(value: &T) -> Result<Value, deser::Error> {
    let text = deser_json::to_string(value)?;
    Value::parse(&text).map_err(|e| {
        deser::Error::new(
            deser::ErrorKind::Custom,
            format!("not JSON: {}", e.message()),
        )
    })
}

/// A [`Value`] as a game type.
pub fn from_value<T: deser::de::DeserializeOwned>(value: &Value) -> Result<T, deser::Error> {
    let text = value.to_string();
    deser_json::from_str::<T>(&text)
}

impl Action {
    /// The action's data as a game type; a mismatch is the phone's fault:
    /// `bad-data` with deser's message (reply it as the error).
    pub fn data_as<T: deser::de::DeserializeOwned>(&self) -> Result<T, PartyError> {
        from_value(self.data()).map_err(|e| PartyError::new("bad-data", e.to_string()))
    }
}

impl State {
    /// [`set`](State::set) from a game type (it must serialize to an
    /// object).
    pub fn set_typed<T: deser::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), deser::Error> {
        let v = to_value(value)?;
        if !matches!(v, Value::Object(_)) {
            return Err(not_an_object());
        }
        self.set(v);
        Ok(())
    }

    /// [`replace`](State::replace) from a game type (an object).
    pub fn replace_typed<T: deser::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), deser::Error> {
        let v = to_value(value)?;
        if !matches!(v, Value::Object(_)) {
            return Err(not_an_object());
        }
        self.replace(v);
        Ok(())
    }

    /// The state as a game type.
    pub fn get_typed<T: deser::de::DeserializeOwned>(&self) -> Result<T, deser::Error> {
        from_value(&Value::Object(self.get().clone()))
    }
}

fn not_an_object() -> deser::Error {
    deser::Error::new(
        deser::ErrorKind::InvalidType,
        "party state must serialize to an object",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[derive(Debug, PartialEq, deser::Serialize, deser::Deserialize)]
    #[deser(rename_all = "camelCase")]
    struct Answer {
        text: String,
        #[deser(default)]
        lie_for_me: bool,
    }

    #[derive(Debug, PartialEq, deser::Serialize, deser::Deserialize)]
    #[deser(tag = "phase", rename_all = "kebab-case")]
    enum Room {
        Lobby { players: u32 },
        Draw { deadline: u64 },
    }

    #[test]
    fn round_trips() {
        let a: Answer = from_value(&json!({"text": "a nervous cactus"})).unwrap();
        assert_eq!(
            a,
            Answer {
                text: "a nervous cactus".into(),
                lie_for_me: false
            }
        );
        assert_eq!(
            to_value(&a).unwrap(),
            json!({"text": "a nervous cactus", "lieForMe": false})
        );
        let r = to_value(&Room::Draw {
            deadline: 1_700_000_000_000,
        })
        .unwrap();
        assert_eq!(
            r,
            json!({"phase": "draw", "deadline": 1_700_000_000_000u64})
        );
        let mut s = State::default();
        s.set_typed(&Room::Lobby { players: 3 }).unwrap();
        assert_eq!(s.get_typed::<Room>().unwrap(), Room::Lobby { players: 3 });
        assert!(s.set_typed(&5u32).is_err());
        assert!(from_value::<Answer>(&json!({"txt": 1})).is_err());
    }
}

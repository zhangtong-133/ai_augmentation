#![forbid(unsafe_code)]

use std::fmt::{self, Display};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

string_id!(UserId);
string_id!(DocumentId);
string_id!(ConversationId);
string_id!(TaskId);
string_id!(SkillId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentState {
    Idle,
    Planning,
    ExecutingTool,
    WaitingForModel,
    PersistingMemory,
    Failed,
}

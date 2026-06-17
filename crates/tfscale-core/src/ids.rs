use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! define_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, Uuid::now_v7().simple()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

define_id!(AuthKeyId, "ak");
define_id!(DeviceId, "dev");
define_id!(NetworkId, "net");
define_id!(OrganizationId, "org");

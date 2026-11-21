//! 文字列と 1 対 1 に対応する enum を定義するマクロ。
//!
//! DB の列や JSON に文字列として保存する値（状態・履歴の種類・設定のキーなど）を、
//! コードの中では enum として扱うために使う。対応表は 1 か所に書くだけで、
//! 文字列への変換・文字列からの読み取り・全種類の一覧・SQLite との読み書き・JSON との読み書きが
//! すべてそこから作られる。種類を足しても、変換の書き漏れは起きない。
//!
//! ```ignore
//! str_enum! {
//!     /// タスクの状態
//!     pub enum TaskStatus {
//!         Todo => "todo",
//!         Doing => "doing",
//!     }
//! }
//! ```

macro_rules! str_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        #[allow(dead_code)] // 一覧や読み取りを使わない enum もある
        impl $name {
            /// すべての種類（定義した順）
            pub const ALL: &'static [$name] = &[$( $name::$variant ),+];

            /// 保存するときの文字列
            pub fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $text ),+
                }
            }

            /// 保存された文字列から読む（知らない文字列なら None）
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $( $text => Some($name::$variant), )+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(rusqlite::types::ToSqlOutput::from(self.as_str()))
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                let s = value.as_str()?;
                // 知らない値を黙って既定値にしない（DB が壊れている・新しい版で書かれた、などに気づけるように）
                $name::parse(s).ok_or_else(|| {
                    rusqlite::types::FromSqlError::Other(
                        format!(concat!(stringify!($name), " として読めない値です: {}"), s).into(),
                    )
                })
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(deserializer)?;
                $name::parse(&s).ok_or_else(|| {
                    let expected: Vec<&str> = $name::ALL.iter().map(|v| v.as_str()).collect();
                    serde::de::Error::custom(format!(
                        "{} は {} のどれかにしてください",
                        s,
                        expected.join(" / ")
                    ))
                })
            }
        }
    };
}

use serde::{Deserialize, Deserializer, Serialize, de::Error};
use whichlang::Lang;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerLanguage {
    #[default]
    Auto,
    En,
    Fr,
    De,
    Es,
    It,
    Pt,
    Ja,
    Zh,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerLanguageOption {
    code: String,
    name: String,
    native_name: String,
}

impl AnswerLanguage {
    const ALL: [Self; 9] = [
        Self::Auto,
        Self::En,
        Self::Fr,
        Self::De,
        Self::Es,
        Self::It,
        Self::Pt,
        Self::Ja,
        Self::Zh,
    ];

    const fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Auto => ("auto", "Automatic", "Automatic"),
            Self::En => ("en", "English", "English"),
            Self::Fr => ("fr", "French", "Français"),
            Self::De => ("de", "German", "Deutsch"),
            Self::Es => ("es", "Spanish", "Español"),
            Self::It => ("it", "Italian", "Italiano"),
            Self::Pt => ("pt", "Portuguese", "Português"),
            Self::Ja => ("ja", "Japanese", "日本語"),
            Self::Zh => ("zh", "Simplified Chinese", "简体中文"),
        }
    }

    pub(crate) fn options() -> Vec<AnswerLanguageOption> {
        Self::ALL
            .into_iter()
            .map(Self::labels)
            .map(|(code, name, native_name)| AnswerLanguageOption {
                code: code.to_owned(),
                name: name.to_owned(),
                native_name: native_name.to_owned(),
            })
            .collect()
    }

    #[must_use]
    pub const fn legacy(self) -> Self {
        match self {
            Self::En | Self::Fr => self,
            _ => Self::Auto,
        }
    }

    pub(super) fn deserialize_legacy<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let language = Self::deserialize(deserializer)?;
        if language.legacy() != language {
            return Err(D::Error::custom(
                "Use answerLocale for additional languages",
            ));
        }
        Ok(language)
    }

    #[must_use]
    pub(crate) const fn code(self) -> &'static str {
        self.labels().0
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.code() == code)
    }

    pub(super) const fn instruction(self) -> &'static str {
        match self {
            Self::Auto => "same language as the question",
            language => language.labels().1,
        }
    }

    pub(super) const fn resolve(self, detected: Option<Lang>) -> Self {
        if !matches!(self, Self::Auto) {
            return self;
        }
        match detected {
            Some(Lang::Eng) => Self::En,
            Some(Lang::Fra) => Self::Fr,
            Some(Lang::Deu) => Self::De,
            Some(Lang::Spa) => Self::Es,
            Some(Lang::Ita) => Self::It,
            Some(Lang::Por) => Self::Pt,
            Some(Lang::Jpn) => Self::Ja,
            // Automatic mode leaves the choice of Chinese script up to the question.
            _ => Self::Auto,
        }
    }
}

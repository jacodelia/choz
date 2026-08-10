//! Interface language.
//!
//! `t("KEY")` returns the string for the language selected in Settings, falling
//! back to the English key itself when a translation is missing — so an
//! untranslated string shows in English instead of blowing up or showing a key.
//!
//! Keys *are* the English text: that keeps the call sites readable
//! (`t("RACK")`) and makes the fallback the right thing by construction.

use std::sync::atomic::{AtomicU8, Ordering};

/// Languages choz ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    En,
    Es,
    Pt,
    Fr,
    It,
    De,
    Ru,
    Ja,
    Zh,
}

impl Lang {
    pub const ALL: &'static [Lang] = &[
        Lang::En,
        Lang::Es,
        Lang::Pt,
        Lang::Fr,
        Lang::It,
        Lang::De,
        Lang::Ru,
        Lang::Ja,
        Lang::Zh,
    ];

    /// Name in the language itself, as language pickers do it.
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Es => "Espa\u{00f1}ol",
            Lang::Pt => "Portugu\u{ea}s",
            Lang::Fr => "Fran\u{e7}ais",
            Lang::It => "Italiano",
            Lang::De => "Deutsch",
            Lang::Ru => "\u{420}\u{443}\u{441}\u{441}\u{43a}\u{438}\u{439}",
            Lang::Ja => "\u{65e5}\u{672c}\u{8a9e}",
            Lang::Zh => "\u{4e2d}\u{6587}",
        }
    }

    /// Two-letter tag, used in the config file and by [`Self::from_env`].
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
            Lang::Pt => "pt",
            Lang::Fr => "fr",
            Lang::It => "it",
            Lang::De => "de",
            Lang::Ru => "ru",
            Lang::Ja => "ja",
            Lang::Zh => "zh",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let c = code.get(..2).unwrap_or(code).to_lowercase();
        Lang::ALL.iter().copied().find(|l| l.code() == c)
    }

    /// Language from `$LC_ALL` / `$LC_MESSAGES` / `$LANG`, English otherwise.
    pub fn from_env() -> Lang {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .find_map(|v| Lang::from_code(&v))
            .unwrap_or(Lang::En)
    }

    fn index(self) -> usize {
        Lang::ALL.iter().position(|l| *l == self).unwrap_or(0)
    }
}

/// Current language, as an index into [`Lang::ALL`]. An atomic keeps `t()`
/// callable from anywhere in the UI without threading a context through every
/// draw function.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(lang: Lang) {
    CURRENT.store(lang.index() as u8, Ordering::Relaxed);
}

pub fn language() -> Lang {
    Lang::ALL
        .get(CURRENT.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or(Lang::En)
}

/// Translations, in [`Lang::ALL`] order after the English key.
/// `""` means "not translated" and falls back to the key.
type Row = (&'static str, [&'static str; 8]);

/// The UI strings worth translating: panel titles, buttons, hints and the
/// modal chrome. Anything not listed here stays English.
#[rustfmt::skip]
static TABLE: &[Row] = &[
    //  key            es              pt              fr              it              de              ru                  ja              zh
    ("INPUTS",       ["ENTRADAS",    "ENTRADAS",     "ENTR\u{c9}ES",  "INGRESSI",     "EING\u{c4}NGE", "\u{412}\u{425}\u{41e}\u{414}\u{42b}", "\u{5165}\u{529b}", "\u{8f93}\u{5165}"]),
    ("RACK",         ["RACK",        "RACK",         "RACK",         "RACK",         "RACK",         "\u{420}\u{415}\u{419}\u{41a}", "\u{30e9}\u{30c3}\u{30af}", "\u{673a}\u{67b6}"]),
    ("TRANSPORT",    ["TRANSPORTE",  "TRANSPORTE",   "TRANSPORT",    "TRASPORTO",    "TRANSPORT",    "\u{422}\u{420}\u{410}\u{41d}\u{421}\u{41f}\u{41e}\u{420}\u{422}", "\u{30c8}\u{30e9}\u{30f3}\u{30b9}\u{30dd}\u{30fc}\u{30c8}", "\u{8d70}\u{5e26}"]),
    ("SETTINGS",     ["AJUSTES",     "AJUSTES",      "R\u{c9}GLAGES", "IMPOSTAZIONI", "EINSTELLUNGEN", "\u{41d}\u{410}\u{421}\u{422}\u{420}\u{41e}\u{419}\u{41a}\u{418}", "\u{8a2d}\u{5b9a}", "\u{8bbe}\u{7f6e}"]),
    ("EDIT",         ["EDITAR",      "EDITAR",       "\u{c9}DITION",  "MODIFICA",     "BEARBEITEN",   "\u{41f}\u{420}\u{410}\u{412}\u{41a}\u{410}", "\u{7de8}\u{96c6}", "\u{7f16}\u{8f91}"]),
    ("COLOR",        ["COLOR",       "COR",          "COULEUR",      "COLORE",       "FARBE",        "\u{426}\u{412}\u{415}\u{422}", "\u{8272}", "\u{989c}\u{8272}"]),
    ("FILE",         ["ARCHIVO",     "ARQUIVO",      "FICHIER",      "FILE",         "DATEI",        "\u{424}\u{410}\u{419}\u{41b}", "\u{30d5}\u{30a1}\u{30a4}\u{30eb}", "\u{6587}\u{4ef6}"]),
    ("HELP",         ["AYUDA",       "AJUDA",        "AIDE",         "AIUTO",        "HILFE",        "\u{421}\u{41f}\u{420}\u{410}\u{412}\u{41a}\u{410}", "\u{30d8}\u{30eb}\u{30d7}", "\u{5e2e}\u{52a9}"]),
    ("SOURCE",       ["FUENTE",      "FONTE",        "SOURCE",       "SORGENTE",     "QUELLE",       "\u{418}\u{421}\u{422}\u{41e}\u{427}\u{41d}\u{418}\u{41a}", "\u{97f3}\u{6e90}", "\u{97f3}\u{6e90}"]),
    ("BANK/PRESET",  ["BANCO/PRESET", "BANCO/PRESET", "BANQUE/PRESET", "BANCO/PRESET", "BANK/PRESET", "\u{411}\u{410}\u{41d}\u{41a}/\u{41f}\u{420}\u{415}\u{421}\u{415}\u{422}", "\u{30d0}\u{30f3}\u{30af}", "\u{97f3}\u{8272}\u{5e93}"]),
    ("MIDI LEARN",   ["APRENDER MIDI", "APRENDER MIDI", "APPRENTISSAGE MIDI", "APPRENDI MIDI", "MIDI LERNEN", "MIDI \u{41e}\u{411}\u{423}\u{427}\u{415}\u{41d}\u{418}\u{415}", "MIDI\u{30e9}\u{30fc}\u{30f3}", "MIDI\u{5b66}\u{4e60}"]),
    ("SCAN INPUTS",  ["ESCANEAR ENTRADAS", "ESCANEAR ENTRADAS", "SCANNER LES ENTR\u{c9}ES", "SCANSIONA INGRESSI", "EING\u{c4}NGE SUCHEN", "\u{421}\u{41a}\u{410}\u{41d} \u{412}\u{425}\u{41e}\u{414}\u{41e}\u{412}", "\u{5165}\u{529b}\u{3092}\u{30b9}\u{30ad}\u{30e3}\u{30f3}", "\u{626b}\u{63cf}\u{8f93}\u{5165}"]),
    ("FX CHAIN",     ["CADENA FX",   "CADEIA FX",    "CHA\u{ce}NE FX", "CATENA FX",   "FX-KETTE",     "\u{426}\u{415}\u{41f}\u{4c}\u{41a}\u{410} FX", "FX\u{30c1}\u{30a7}\u{30fc}\u{30f3}", "FX \u{94fe}"]),
    ("ADD FX",       ["A\u{d1}ADIR FX", "ADICIONAR FX", "AJOUTER FX", "AGGIUNGI FX", "FX HINZUF\u{dc}GEN", "\u{414}\u{41e}\u{411}\u{410}\u{412}\u{418}\u{422}\u{42c} FX", "FX\u{8ffd}\u{52a0}", "\u{6dfb}\u{52a0} FX"]),
    ("SLOT",         ["RANURA",      "SLOT",         "SLOT",         "SLOT",         "SLOT",         "\u{421}\u{41b}\u{41e}\u{422}", "\u{30b9}\u{30ed}\u{30c3}\u{30c8}", "\u{63d2}\u{69fd}"]),
    ("SELECT",       ["ELEGIR",      "SELECIONAR",   "CHOISIR",      "SCEGLI",       "W\u{c4}HLEN",   "\u{412}\u{42b}\u{411}\u{420}\u{410}\u{422}\u{42c}", "\u{9078}\u{629e}", "\u{9009}\u{62e9}"]),
    ("CANCEL",       ["CANCELAR",    "CANCELAR",     "ANNULER",      "ANNULLA",      "ABBRECHEN",    "\u{41e}\u{422}\u{41c}\u{415}\u{41d}\u{410}", "\u{30ad}\u{30e3}\u{30f3}\u{30bb}\u{30eb}", "\u{53d6}\u{6d88}"]),
    ("EDIT",         ["EDITAR",      "EDITAR",       "\u{c9}DITER",   "MODIFICA",     "BEARBEITEN",   "\u{41f}\u{420}\u{410}\u{412}\u{41a}\u{410}", "\u{7de8}\u{96c6}", "\u{7f16}\u{8f91}"]),
    ("ADD",          ["A\u{d1}ADIR", "ADICIONAR",    "AJOUTER",      "AGGIUNGI",     "HINZUF\u{dc}GEN", "\u{414}\u{41e}\u{411}\u{410}\u{412}\u{418}\u{422}\u{42c}", "\u{8ffd}\u{52a0}", "\u{6dfb}\u{52a0}"]),
    ("BROWSE",       ["EXAMINAR",    "PROCURAR",     "PARCOURIR",    "SFOGLIA",      "DURCHSUCHEN",  "\u{41e}\u{411}\u{417}\u{41e}\u{420}", "\u{53c2}\u{7167}", "\u{6d4f}\u{89c8}"]),
    ("REMOVE",       ["QUITAR",      "REMOVER",      "SUPPRIMER",    "RIMUOVI",      "ENTFERNEN",    "\u{423}\u{414}\u{410}\u{41b}\u{418}\u{422}\u{42c}", "\u{524a}\u{9664}", "\u{79fb}\u{9664}"]),
    ("DEFAULTS",     ["POR DEFECTO", "PADR\u{d5}ES",  "PAR D\u{c9}FAUT", "PREDEFINITI", "STANDARD",   "\u{41f}\u{41e} \u{423}\u{41c}\u{41e}\u{41b}\u{427}\u{410}\u{41d}\u{418}\u{42e}", "\u{521d}\u{671f}\u{5024}", "\u{9ed8}\u{8ba4}\u{503c}"]),
    ("PLUGIN PATHS", ["RUTAS DE PLUGINS", "CAMINHOS DE PLUGINS", "CHEMINS DES PLUGINS", "PERCORSI PLUGIN", "PLUGIN-PFADE", "\u{41f}\u{423}\u{422}\u{418} \u{41f}\u{41b}\u{410}\u{413}\u{418}\u{41d}\u{41e}\u{412}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{30d1}\u{30b9}", "\u{63d2}\u{4ef6}\u{8def}\u{5f84}"]),
    ("TEXT COLOR",   ["COLOR DE TEXTO", "COR DO TEXTO", "COULEUR DU TEXTE", "COLORE TESTO", "TEXTFARBE", "\u{426}\u{412}\u{415}\u{422} \u{422}\u{415}\u{41a}\u{421}\u{422}\u{410}", "\u{6587}\u{5b57}\u{8272}", "\u{6587}\u{5b57}\u{989c}\u{8272}"]),
    ("LANGUAGE",     ["IDIOMA",      "IDIOMA",       "LANGUE",       "LINGUA",       "SPRACHE",      "\u{42f}\u{417}\u{42b}\u{41a}", "\u{8a00}\u{8a9e}", "\u{8bed}\u{8a00}"]),
    ("PLAY",         ["TOCAR",       "TOCAR",        "LIRE",         "RIPRODUCI",    "PLAY",         "\u{41f}\u{423}\u{421}\u{41a}", "\u{518d}\u{751f}", "\u{64ad}\u{653e}"]),
    ("STOP",         ["PARAR",       "PARAR",        "STOP",         "STOP",         "STOPP",        "\u{421}\u{422}\u{41e}\u{41f}", "\u{505c}\u{6b62}", "\u{505c}\u{6b62}"]),
    ("STOPPED",      ["DETENIDO",    "PARADO",       "ARR\u{ca}T\u{c9}", "FERMO",     "GESTOPPT",     "\u{41e}\u{421}\u{422}\u{410}\u{41d}\u{41e}\u{412}\u{41b}\u{415}\u{41d}", "\u{505c}\u{6b62}\u{4e2d}", "\u{5df2}\u{505c}\u{6b62}"]),
    ("PLAYING",      ["REPRODUCIENDO", "REPRODUZINDO", "LECTURE",    "IN RIPRODUZIONE", "L\u{c4}UFT", "\u{412}\u{41e}\u{421}\u{41f}\u{420}\u{41e}\u{418}\u{417}\u{412}\u{415}\u{414}\u{415}\u{41d}\u{418}\u{415}", "\u{518d}\u{751f}\u{4e2d}", "\u{64ad}\u{653e}\u{4e2d}"]),
    ("OUT",          ["SALIDA",      "SA\u{cd}DA",    "SORTIE",       "USCITA",       "AUSGANG",      "\u{412}\u{42b}\u{425}\u{41e}\u{414}", "\u{51fa}\u{529b}", "\u{8f93}\u{51fa}"]),
    ("INSTR",        ["INSTR",       "INSTR",        "INSTR",        "STRUM",        "INSTR",        "\u{418}\u{41d}\u{421}\u{422}\u{420}", "\u{6a5f}\u{5668}", "\u{4e50}\u{5668}"]),
    ("BANK",         ["BANCO",       "BANCO",        "BANQUE",       "BANCO",        "BANK",         "\u{411}\u{410}\u{41d}\u{41a}", "\u{30d0}\u{30f3}\u{30af}", "\u{97f3}\u{8272}\u{5e93}"]),
    ("VOL",          ["VOL",         "VOL",          "VOL",          "VOL",          "LAUT",         "\u{413}\u{420}\u{41c}", "\u{97f3}\u{91cf}", "\u{97f3}\u{91cf}"]),
    ("PAN",          ["PAN",         "PAN",          "PAN",          "PAN",          "PAN",          "\u{41f}\u{410}\u{41d}", "\u{5b9a}\u{4f4d}", "\u{58f0}\u{50cf}"]),
    ("MUTE",         ["MUDO",        "MUDO",         "MUET",         "MUTO",         "STUMM",        "\u{422}\u{418}\u{428}\u{415}", "\u{30df}\u{30e5}\u{30fc}\u{30c8}", "\u{9759}\u{97f3}"]),
    ("SOLO",         ["SOLO",        "SOLO",         "SOLO",         "SOLO",         "SOLO",         "\u{421}\u{41e}\u{41b}\u{41e}", "\u{30bd}\u{30ed}", "\u{72ec}\u{594f}"]),
];

/// The interface string for `key` in the current language.
pub fn t(key: &str) -> &str {
    lookup(language(), key)
}

/// `key` in `lang`, falling back to the key (which is the English text).
/// Pure, so the tests never have to touch the global.
pub fn lookup(lang: Lang, key: &str) -> &str {
    if lang == Lang::En {
        return key;
    }
    // Column 0 of the row is the first non-English language.
    let col = lang.index().saturating_sub(1);
    TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .and_then(|(_, tr)| tr.get(col).copied())
        .filter(|s| !s.is_empty())
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row has one translation per non-English language, and none of them
    /// is accidentally empty (an empty cell silently falls back to English).
    #[test]
    fn the_table_is_complete() {
        for (key, row) in TABLE {
            assert_eq!(
                row.len(),
                Lang::ALL.len() - 1,
                "{key} has the wrong column count"
            );
            for (i, cell) in row.iter().enumerate() {
                assert!(
                    !cell.is_empty(),
                    "{key} is missing {}",
                    Lang::ALL[i + 1].code()
                );
            }
        }
    }

    /// Checked through `lookup`, not `t`: the current language is process-wide
    /// state and switching it here would leak into the rendering tests.
    #[test]
    fn lookup_translates_and_falls_back() {
        assert_eq!(lookup(Lang::Es, "SETTINGS"), "AJUSTES");
        assert_eq!(lookup(Lang::Ja, "LANGUAGE"), "\u{8a00}\u{8a9e}");
        assert_eq!(
            lookup(Lang::Es, "Not in the table"),
            "Not in the table",
            "unknown keys stay as-is"
        );
        assert_eq!(
            lookup(Lang::En, "SETTINGS"),
            "SETTINGS",
            "English is the key itself"
        );
    }

    #[test]
    fn languages_round_trip_through_their_code() {
        for &l in Lang::ALL {
            assert_eq!(Lang::from_code(l.code()), Some(l));
        }
        assert_eq!(
            Lang::from_code("es_AR.UTF-8"),
            Some(Lang::Es),
            "locale strings work"
        );
        assert_eq!(Lang::from_code("xx"), None);
    }
}

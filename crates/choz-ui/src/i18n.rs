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
    ("AUDIO",        ["AUDIO",       "\u{c1}UDIO",    "AUDIO",        "AUDIO",        "AUDIO",        "\u{410}\u{423}\u{414}\u{418}\u{41e}", "\u{30aa}\u{30fc}\u{30c7}\u{30a3}\u{30aa}", "\u{97f3}\u{9891}"]),
    ("THEME",        ["TEMA",        "TEMA",         "TH\u{c8}ME",   "TEMA",         "THEMA",        "\u{422}\u{415}\u{41c}\u{410}", "\u{30c6}\u{30fc}\u{30de}", "\u{4e3b}\u{9898}"]),
    ("AUDIO IN",     ["ENTRADA AUDIO", "ENTRADA \u{c1}UDIO", "ENTR\u{c9}E AUDIO", "INGRESSO AUDIO", "AUDIO-EINGANG", "\u{410}\u{423}\u{414}\u{418}\u{41e} \u{412}\u{425}\u{41e}\u{414}", "\u{30aa}\u{30fc}\u{30c7}\u{30a3}\u{30aa}\u{5165}\u{529b}", "\u{97f3}\u{9891}\u{8f93}\u{5165}"]),
    ("NOTE IN",      ["ENTRADA NOTAS", "ENTRADA NOTAS", "ENTR\u{c9}E NOTES", "INGRESSO NOTE", "NOTEN-EINGANG", "\u{412}\u{425}\u{41e}\u{414} \u{41d}\u{41e}\u{422}", "\u{30ce}\u{30fc}\u{30c8}\u{5165}\u{529b}", "\u{97f3}\u{7b26}\u{8f93}\u{5165}"]),
    ("IN",           ["ENT",         "ENT",          "ENT",          "IN",           "EIN",          "\u{412}\u{425}", "\u{5165}", "\u{5165}"]),
    ("DEVICE",       ["DISPOSITIVO", "DISPOSITIVO",  "P\u{c9}RIPH\u{c9}RIQUE", "DISPOSITIVO", "GER\u{c4}T", "\u{423}\u{421}\u{422}\u{420}\u{41e}\u{419}\u{421}\u{422}\u{412}\u{41e}", "\u{30c7}\u{30d0}\u{30a4}\u{30b9}", "\u{8bbe}\u{5907}"]),
    ("INSTRUMENT",   ["INSTRUMENTO", "INSTRUMENTO",  "INSTRUMENT",   "STRUMENTO",    "INSTRUMENT",   "\u{418}\u{41d}\u{421}\u{422}\u{420}\u{423}\u{41c}\u{415}\u{41d}\u{422}", "\u{697d}\u{5668}", "\u{4e50}\u{5668}"]),
    ("(instrument)", ["(instrumento)", "(instrumento)", "(instrument)", "(strumento)", "(Instrument)", "(\u{438}\u{43d}\u{441}\u{442}\u{440}\u{443}\u{43c}\u{435}\u{43d}\u{442})", "(\u{697d}\u{5668})", "(\u{4e50}\u{5668})"]),
    ("PRESET",       ["PRESET",      "PRESET",       "PR\u{c9}R\u{c9}GLAGE", "PRESET", "PRESET",   "\u{41f}\u{420}\u{415}\u{421}\u{415}\u{422}", "\u{30d7}\u{30ea}\u{30bb}\u{30c3}\u{30c8}", "\u{9884}\u{8bbe}"]),
    ("PICK BANK",    ["ELEGIR BANCO", "ESCOLHER BANCO", "CHOISIR BANQUE", "SCEGLI BANCO", "BANK WÄHLEN", "ВЫБРАТЬ БАНК", "バンク選択", "选择音色库"]),
    ("BANK FOLDER",  ["CARPETA DEL BANCO", "PASTA DO BANCO", "DOSSIER DE BANQUE", "CARTELLA BANCO", "BANK-ORDNER", "ПАПКА БАНКА", "バンクフォルダ", "音色库文件夹"]),
    ("ARP",          ["ARP",         "ARP",          "ARP",          "ARP",          "ARP",          "\u{410}\u{420}\u{41f}", "\u{30a2}\u{30eb}\u{30da}", "\u{7434}\u{97f3}"]),
    ("SENS",         ["SENS",        "SENS",         "SENS",         "SENS",         "EMPF",         "\u{427}\u{423}\u{412}", "\u{611f}\u{5ea6}", "\u{7075}\u{654f}"]),
    ("RACK ONLY",    ["SOLO RACK",   "S\u{d3} RACK",  "RACK SEUL",    "SOLO RACK",    "NUR RACK",     "\u{422}\u{41e}\u{41b}\u{42c}\u{41a}\u{41e} \u{420}\u{415}\u{419}\u{41a}", "\u{30e9}\u{30c3}\u{30af}\u{306e}\u{307f}", "\u{4ec5}\u{673a}\u{67b6}"]),
    ("Open WAV\u{2026}", ["Abrir WAV\u{2026}", "Abrir WAV\u{2026}", "Ouvrir WAV\u{2026}", "Apri WAV\u{2026}", "WAV \u{f6}ffnen\u{2026}", "\u{41e}\u{442}\u{43a}\u{440}\u{44b}\u{442}\u{44c} WAV\u{2026}", "WAV\u{3092}\u{958b}\u{304f}\u{2026}", "\u{6253}\u{5f00} WAV\u{2026}"]),
    ("Open SF2\u{2026}", ["Abrir SF2\u{2026}", "Abrir SF2\u{2026}", "Ouvrir SF2\u{2026}", "Apri SF2\u{2026}", "SF2 \u{f6}ffnen\u{2026}", "\u{41e}\u{442}\u{43a}\u{440}\u{44b}\u{442}\u{44c} SF2\u{2026}", "SF2\u{3092}\u{958b}\u{304f}\u{2026}", "\u{6253}\u{5f00} SF2\u{2026}"]),
    ("ALL BANKS", ["TODOS LOS BANCOS", "TODOS OS BANCOS", "TOUTES LES BANQUES", "TUTTI I BANCHI", "ALLE B\u{c4}NKE", "\u{412}\u{421}\u{415} \u{411}\u{410}\u{41d}\u{41a}\u{418}", "\u{3059}\u{3079}\u{3066}\u{306e}\u{30d0}\u{30f3}\u{30af}", "\u{5168}\u{90e8}\u{97f3}\u{8272}\u{5e93}"]),
    ("OVERWRITE PROJECT?", ["\u{bf}SOBRESCRIBIR PROYECTO?", "SOBRESCREVER PROJETO?", "\u{c9}CRASER LE PROJET\u{a0}?", "SOVRASCRIVERE IL PROGETTO?", "PROJEKT \u{dc}BERSCHREIBEN?", "\u{41f}\u{415}\u{420}\u{415}\u{417}\u{410}\u{41f}\u{418}\u{421}\u{410}\u{422}\u{42c} \u{41f}\u{420}\u{41e}\u{415}\u{41a}\u{422}?", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{4e0a}\u{66f8}\u{304d}\u{ff1f}", "\u{8986}\u{76d6}\u{9879}\u{76ee}\u{ff1f}"]),
    ("OVERWRITE", ["SOBRESCRIBIR", "SOBRESCREVER", "\u{c9}CRASER", "SOVRASCRIVI", "\u{dc}BERSCHREIBEN", "\u{41f}\u{415}\u{420}\u{415}\u{417}\u{410}\u{41f}\u{418}\u{421}\u{410}\u{422}\u{42c}", "\u{4e0a}\u{66f8}\u{304d}", "\u{8986}\u{76d6}"]),
    ("RENAME INSTEAD", ["CAMBIAR EL NOMBRE", "RENOMEAR", "RENOMMER", "RINOMINA", "UMBENENNEN", "\u{41f}\u{415}\u{420}\u{415}\u{418}\u{41c}\u{415}\u{41d}\u{41e}\u{412}\u{410}\u{422}\u{42c}", "\u{540d}\u{524d}\u{3092}\u{5909}\u{3048}\u{308b}", "\u{91cd}\u{547d}\u{540d}"]),
    ("Save project", ["Guardar proyecto", "Salvar projeto", "Enregistrer le projet", "Salva progetto", "Projekt speichern", "\u{421}\u{43e}\u{445}\u{440}\u{430}\u{43d}\u{438}\u{442}\u{44c} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{4fdd}\u{5b58}", "\u{4fdd}\u{5b58}\u{9879}\u{76ee}"]),
    ("Save project as\u{2026}", ["Guardar proyecto como\u{2026}", "Salvar projeto como\u{2026}", "Enregistrer le projet sous\u{2026}", "Salva progetto come\u{2026}", "Projekt speichern unter\u{2026}", "\u{421}\u{43e}\u{445}\u{440}\u{430}\u{43d}\u{438}\u{442}\u{44c} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442} \u{43a}\u{430}\u{43a}\u{2026}", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{5225}\u{540d}\u{3067}\u{4fdd}\u{5b58}\u{2026}", "\u{9879}\u{76ee}\u{53e6}\u{5b58}\u{4e3a}\u{2026}"]),
    ("Open project\u{2026}", ["Abrir proyecto\u{2026}", "Abrir projeto\u{2026}", "Ouvrir un projet\u{2026}", "Apri progetto\u{2026}", "Projekt \u{f6}ffnen\u{2026}", "\u{41e}\u{442}\u{43a}\u{440}\u{44b}\u{442}\u{44c} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}\u{2026}", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{958b}\u{304f}\u{2026}", "\u{6253}\u{5f00}\u{9879}\u{76ee}\u{2026}"]),
    ("Import Max patch\u{2026}", ["Importar patch Max\u{2026}", "Importar patch Max\u{2026}", "Importer un patch Max\u{2026}", "Importa patch Max\u{2026}", "Max-Patch importieren\u{2026}", "\u{418}\u{43c}\u{43f}\u{43e}\u{440}\u{442} \u{43f}\u{430}\u{442}\u{447}\u{430} Max\u{2026}", "Max\u{30d1}\u{30c3}\u{30c1}\u{3092}\u{8aad}\u{8fbc}\u{2026}", "\u{5bfc}\u{5165} Max \u{8865}\u{4e01}\u{2026}"]),
    ("Quit",         ["Salir",       "Sair",         "Quitter",      "Esci",         "Beenden",      "\u{412}\u{44b}\u{445}\u{43e}\u{434}", "\u{7d42}\u{4e86}", "\u{9000}\u{51fa}"]),
    ("Settings\u{2026}", ["Ajustes\u{2026}", "Ajustes\u{2026}", "R\u{e9}glages\u{2026}", "Impostazioni\u{2026}", "Einstellungen\u{2026}", "\u{41d}\u{430}\u{441}\u{442}\u{440}\u{43e}\u{439}\u{43a}\u{438}\u{2026}", "\u{8a2d}\u{5b9a}\u{2026}", "\u{8bbe}\u{7f6e}\u{2026}"]),
    ("Rescan plugin paths", ["Reescanear rutas de plugins", "Reexaminar caminhos de plugins", "Ranalyser les chemins de plugins", "Riscansiona percorsi plugin", "Plugin-Pfade neu scannen", "\u{41f}\u{435}\u{440}\u{435}\u{441}\u{43a}\u{430}\u{43d} \u{43f}\u{443}\u{442}\u{435}\u{439} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{43e}\u{432}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{30d1}\u{30b9}\u{3092}\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}", "\u{91cd}\u{65b0}\u{626b}\u{63cf}\u{63d2}\u{4ef6}\u{8def}\u{5f84}"]),
    ("About choz",   ["Acerca de choz", "Sobre o choz", "\u{c0} propos de choz", "Informazioni su choz", "\u{dc}ber choz", "\u{41e} choz", "choz \u{306b}\u{3064}\u{3044}\u{3066}", "\u{5173}\u{4e8e} choz"]),
    ("Rescanning plugin paths", ["Reescaneando rutas de plugins", "Reexaminando caminhos de plugins", "Nouvelle analyse des chemins de plugins", "Nuova scansione dei percorsi plugin", "Plugin-Pfade werden neu gescannt", "\u{41f}\u{415}\u{420}\u{415}\u{421}\u{41a}\u{410}\u{41d} \u{41f}\u{423}\u{422}\u{415}\u{419} \u{41f}\u{41b}\u{410}\u{413}\u{418}\u{41d}\u{41e}\u{412}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{30d1}\u{30b9}\u{3092}\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{4e2d}", "\u{6b63}\u{5728}\u{91cd}\u{65b0}\u{626b}\u{63cf}\u{63d2}\u{4ef6}\u{8def}\u{5f84}"]),
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
    /// No key may appear twice. `lookup` takes the first match, so a second row
    /// for the same key is dead weight that reads like a translation — the
    /// duplicate `EDIT` row carried a French string nothing could ever show.
    #[test]
    fn no_key_is_listed_twice() {
        let mut keys: Vec<&str> = TABLE.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(
            keys.len(),
            before,
            "duplicate keys in TABLE; the second copy of each is unreachable"
        );
    }

    /// A key is the English text, so a row whose translation equals the key in
    /// every language is a row doing nothing. Catching it here keeps the table
    /// honest about what is actually translated.
    #[test]
    fn every_row_translates_something() {
        for (key, row) in TABLE {
            assert!(
                row.iter().any(|t| t != key),
                "{key:?} is identical in all nine languages; drop the row or \
                 translate it"
            );
        }
    }
}

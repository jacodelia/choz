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
    ("SETTINGS",     ["AJUSTES",     "AJUSTES",      "R\u{c9}GLAGES", "IMPOSTAZIONI", "EINSTELLUNGEN", "\u{41d}\u{410}\u{421}\u{422}\u{420}\u{41e}\u{419}\u{41a}\u{418}", "\u{8a2d}\u{5b9a}", "\u{8bbe}\u{7f6e}"]),
    ("EDIT",         ["EDITAR",      "EDITAR",       "\u{c9}DITION",  "MODIFICA",     "BEARBEITEN",   "\u{41f}\u{420}\u{410}\u{412}\u{41a}\u{410}", "\u{7de8}\u{96c6}", "\u{7f16}\u{8f91}"]),
    ("FILE",         ["ARCHIVO",     "ARQUIVO",      "FICHIER",      "FILE",         "DATEI",        "\u{424}\u{410}\u{419}\u{41b}", "\u{30d5}\u{30a1}\u{30a4}\u{30eb}", "\u{6587}\u{4ef6}"]),
    ("HELP",         ["AYUDA",       "AJUDA",        "AIDE",         "AIUTO",        "HILFE",        "\u{421}\u{41f}\u{420}\u{410}\u{412}\u{41a}\u{410}", "\u{30d8}\u{30eb}\u{30d7}", "\u{5e2e}\u{52a9}"]),
    ("wheel", ["rueda", "roda", "molette", "rotella", "Mausrad", "\u{43a}\u{43e}\u{43b}\u{435}\u{441}\u{43e}", "\u{30db}\u{30a4}\u{30fc}\u{30eb}", "\u{6eda}\u{8f6e}"]),
    ("close", ["cierra", "fecha", "ferme", "chiude", "schlie\u{df}t", "\u{437}\u{430}\u{43a}\u{440}\u{44b}\u{432}\u{430}\u{435}\u{442}", "\u{9589}\u{3058}\u{308b}", "\u{5173}\u{95ed}"]),
    ("TARGET", ["DESTINO", "DESTINO", "CIBLE", "DESTINAZIONE", "ZIEL", "\u{426}\u{415}\u{41b}\u{42c}", "\u{5bfe}\u{8c61}", "\u{76ee}\u{6807}"]),
    ("MIX", ["MIX", "MIX", "MIX", "MIX", "MIX", "\u{41c}\u{418}\u{41a}\u{421}", "\u{30df}\u{30c3}\u{30af}\u{30b9}", "\u{6df7}\u{5408}"]),
    ("SOURCE",       ["FUENTE",      "FONTE",        "SOURCE",       "SORGENTE",     "QUELLE",       "\u{418}\u{421}\u{422}\u{41e}\u{427}\u{41d}\u{418}\u{41a}", "\u{97f3}\u{6e90}", "\u{97f3}\u{6e90}"]),
    ("BANK/PRESET",  ["BANCO/PRESET", "BANCO/PRESET", "BANQUE/PRESET", "BANCO/PRESET", "BANK/PRESET", "\u{411}\u{410}\u{41d}\u{41a}/\u{41f}\u{420}\u{415}\u{421}\u{415}\u{422}", "\u{30d0}\u{30f3}\u{30af}", "\u{97f3}\u{8272}\u{5e93}"]),
    ("MIDI LEARN",   ["APRENDER MIDI", "APRENDER MIDI", "APPRENTISSAGE MIDI", "APPRENDI MIDI", "MIDI LERNEN", "MIDI \u{41e}\u{411}\u{423}\u{427}\u{415}\u{41d}\u{418}\u{415}", "MIDI\u{30e9}\u{30fc}\u{30f3}", "MIDI\u{5b66}\u{4e60}"]),
    ("SCAN INPUTS",  ["ESCANEAR ENTRADAS", "ESCANEAR ENTRADAS", "SCANNER LES ENTR\u{c9}ES", "SCANSIONA INGRESSI", "EING\u{c4}NGE SUCHEN", "\u{421}\u{41a}\u{410}\u{41d} \u{412}\u{425}\u{41e}\u{414}\u{41e}\u{412}", "\u{5165}\u{529b}\u{3092}\u{30b9}\u{30ad}\u{30e3}\u{30f3}", "\u{626b}\u{63cf}\u{8f93}\u{5165}"]),
    ("FX CHAIN",     ["CADENA FX",   "CADEIA FX",    "CHA\u{ce}NE FX", "CATENA FX",   "FX-KETTE",     "\u{426}\u{415}\u{41f}\u{4c}\u{41a}\u{410} FX", "FX\u{30c1}\u{30a7}\u{30fc}\u{30f3}", "FX \u{94fe}"]),
    ("ADD FX",       ["A\u{d1}ADIR FX", "ADICIONAR FX", "AJOUTER FX", "AGGIUNGI FX", "FX HINZUF\u{dc}GEN", "\u{414}\u{41e}\u{411}\u{410}\u{412}\u{418}\u{422}\u{42c} FX", "FX\u{8ffd}\u{52a0}", "\u{6dfb}\u{52a0} FX"]),
    ("SLOT",         ["RANURA",      "SLOT",         "SLOT",         "SLOT",         "SLOT",         "\u{421}\u{41b}\u{41e}\u{422}", "\u{30b9}\u{30ed}\u{30c3}\u{30c8}", "\u{63d2}\u{69fd}"]),
    ("SELECT",       ["ELEGIR",      "SELECIONAR",   "CHOISIR",      "SCEGLI",       "W\u{c4}HLEN",   "\u{412}\u{42b}\u{411}\u{420}\u{410}\u{422}\u{42c}", "\u{9078}\u{629e}", "\u{9009}\u{62e9}"]),
    ("UNSAVED PROJECT", ["PROYECTO SIN GUARDAR", "PROJETO N\u{c3}O SALVO", "PROJET NON ENREGISTR\u{c9}", "PROGETTO NON SALVATO", "PROJEKT NICHT GESPEICHERT", "\u{41f}\u{420}\u{41e}\u{415}\u{41a}\u{422} \u{41d}\u{415} \u{421}\u{41e}\u{425}\u{420}\u{410}\u{41d}\u{415}\u{41d}", "\u{672a}\u{4fdd}\u{5b58}\u{306e}\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}", "\u{9879}\u{76ee}\u{672a}\u{4fdd}\u{5b58}"]),
    ("SAVE AND START A NEW ONE", ["GUARDAR Y EMPEZAR UNO NUEVO", "SALVAR E COME\u{c7}AR UM NOVO", "ENREGISTRER ET EN COMMENCER UN NOUVEAU", "SALVA E INIZIANE UNO NUOVO", "SPEICHERN UND NEU BEGINNEN", "\u{421}\u{41e}\u{425}\u{420}\u{410}\u{41d}\u{418}\u{422}\u{42c} \u{418} \u{41d}\u{410}\u{427}\u{410}\u{422}\u{42c} \u{41d}\u{41e}\u{412}\u{42b}\u{419}", "\u{4fdd}\u{5b58}\u{3057}\u{3066}\u{65b0}\u{898f}\u{4f5c}\u{6210}", "\u{4fdd}\u{5b58}\u{5e76}\u{65b0}\u{5efa}"]),
    ("DISCARD AND START A NEW ONE", ["DESCARTAR Y EMPEZAR UNO NUEVO", "DESCARTAR E COME\u{c7}AR UM NOVO", "ABANDONNER ET EN COMMENCER UN NOUVEAU", "SCARTA E INIZIANE UNO NUOVO", "VERWERFEN UND NEU BEGINNEN", "\u{41e}\u{422}\u{411}\u{420}\u{41e}\u{421}\u{418}\u{422}\u{42c} \u{418} \u{41d}\u{410}\u{427}\u{410}\u{422}\u{42c} \u{41d}\u{41e}\u{412}\u{42b}\u{419}", "\u{7834}\u{68c4}\u{3057}\u{3066}\u{65b0}\u{898f}\u{4f5c}\u{6210}", "\u{653e}\u{5f03}\u{5e76}\u{65b0}\u{5efa}"]),
    ("SAVE AND QUIT", ["GUARDAR Y SALIR", "SALVAR E SAIR", "ENREGISTRER ET QUITTER", "SALVA ED ESCI", "SPEICHERN UND BEENDEN", "\u{421}\u{41e}\u{425}\u{420}\u{410}\u{41d}\u{418}\u{422}\u{42c} \u{418} \u{412}\u{42b}\u{419}\u{422}\u{418}", "\u{4fdd}\u{5b58}\u{3057}\u{3066}\u{7d42}\u{4e86}", "\u{4fdd}\u{5b58}\u{5e76}\u{9000}\u{51fa}"]),
    ("QUIT WITHOUT SAVING", ["SALIR SIN GUARDAR", "SAIR SEM SALVAR", "QUITTER SANS ENREGISTRER", "ESCI SENZA SALVARE", "BEENDEN OHNE ZU SPEICHERN", "\u{412}\u{42b}\u{419}\u{422}\u{418} \u{411}\u{415}\u{417} \u{421}\u{41e}\u{425}\u{420}\u{410}\u{41d}\u{415}\u{41d}\u{418}\u{42f}", "\u{4fdd}\u{5b58}\u{305b}\u{305a}\u{306b}\u{7d42}\u{4e86}", "\u{4e0d}\u{4fdd}\u{5b58}\u{9000}\u{51fa}"]),
    ("this project has never been saved", ["este proyecto nunca se guard\u{f3}", "este projeto nunca foi salvo", "ce projet n\u{2019}a jamais \u{e9}t\u{e9} enregistr\u{e9}", "questo progetto non \u{e8} mai stato salvato", "dieses Projekt wurde nie gespeichert", "\u{44d}\u{442}\u{43e}\u{442} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442} \u{43d}\u{438} \u{440}\u{430}\u{437}\u{443} \u{43d}\u{435} \u{441}\u{43e}\u{445}\u{440}\u{430}\u{43d}\u{44f}\u{43b}\u{441}\u{44f}", "\u{3053}\u{306e}\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{306f}\u{4fdd}\u{5b58}\u{3055}\u{308c}\u{3066}\u{3044}\u{307e}\u{305b}\u{3093}", "\u{6b64}\u{9879}\u{76ee}\u{4ece}\u{672a}\u{4fdd}\u{5b58}"]),
    ("LEARN", ["APRENDER", "APRENDER", "APPRENDRE", "APPRENDI", "LERNEN", "\u{41e}\u{411}\u{423}\u{427}\u{415}\u{41d}\u{418}\u{415}", "\u{30e9}\u{30fc}\u{30f3}", "\u{5b66}\u{4e60}"]),
    ("SEARCH", ["BUSCAR", "BUSCAR", "RECHERCHE", "CERCA", "SUCHE", "\u{41f}\u{41e}\u{418}\u{421}\u{41a}", "\u{691c}\u{7d22}", "\u{641c}\u{7d22}"]),
    ("type to search", ["escribe para buscar", "digite para buscar", "tapez pour rechercher", "digita per cercare", "tippen zum Suchen", "\u{432}\u{432}\u{435}\u{434}\u{438}\u{442}\u{435} \u{434}\u{43b}\u{44f} \u{43f}\u{43e}\u{438}\u{441}\u{43a}\u{430}", "\u{5165}\u{529b}\u{3057}\u{3066}\u{691c}\u{7d22}", "\u{8f93}\u{5165}\u{4ee5}\u{641c}\u{7d22}"]),
    ("back to the whole list", ["volver a la lista completa", "voltar \u{e0} lista completa", "revenir \u{e0} la liste compl\u{e8}te", "torna all\u{2019}elenco completo", "zur\u{fc}ck zur ganzen Liste", "\u{432}\u{435}\u{440}\u{43d}\u{443}\u{442}\u{44c}\u{441}\u{44f} \u{43a}\u{43e} \u{432}\u{441}\u{435}\u{43c}\u{443} \u{441}\u{43f}\u{438}\u{441}\u{43a}\u{443}", "\u{5168}\u{30ea}\u{30b9}\u{30c8}\u{306b}\u{623b}\u{308b}", "\u{8fd4}\u{56de}\u{5b8c}\u{6574}\u{5217}\u{8868}"]),
    ("rescan plugins", ["reescanear plugins", "reescanear plugins", "rescanner les greffons", "riscansiona i plugin", "Plugins neu suchen", "\u{43f}\u{435}\u{440}\u{435}\u{441}\u{43a}\u{430}\u{43d}\u{438}\u{440}\u{43e}\u{432}\u{430}\u{442}\u{44c} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{44b}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}", "\u{91cd}\u{65b0}\u{626b}\u{63cf}\u{63d2}\u{4ef6}"]),
    ("TAB", ["PESTA\u{d1}A", "ABA", "ONGLET", "SCHEDA", "TAB", "\u{412}\u{41a}\u{41b}\u{410}\u{414}\u{41a}\u{410}", "\u{30bf}\u{30d6}", "\u{6807}\u{7b7e}"]),
    ("INPUT", ["ENTRADA", "ENTRADA", "ENTR\u{c9}E", "INGRESSO", "EINGANG", "\u{412}\u{425}\u{41e}\u{414}", "\u{5165}\u{529b}", "\u{8f93}\u{5165}"]),
    ("REPLACE IT", ["REEMPLAZARLO", "SUBSTITUIR", "LE REMPLACER", "SOSTITUISCILO", "ERSETZEN", "\u{417}\u{410}\u{41c}\u{415}\u{41d}\u{418}\u{422}\u{42c}", "\u{7f6e}\u{304d}\u{63db}\u{3048}\u{308b}", "\u{66ff}\u{6362}"]),
    ("KEEP WHAT IS THERE", ["DEJAR LO QUE HAY", "MANTER O QUE EST\u{c1}", "GARDER CE QUI EST L\u{c0}", "MANTIENI QUELLO CHE C\u{2019}\u{c8}", "BEIBEHALTEN", "\u{41e}\u{421}\u{422}\u{410}\u{412}\u{418}\u{422}\u{42c} \u{41a}\u{410}\u{41a} \u{415}\u{421}\u{422}\u{42c}", "\u{305d}\u{306e}\u{307e}\u{307e}\u{306b}\u{3059}\u{308b}", "\u{4fdd}\u{6301}\u{4e0d}\u{53d8}"]),
    ("HOLDS", ["TIENE", "TEM", "CONTIENT", "CONTIENE", "HAT", "\u{421}\u{41e}\u{414}\u{415}\u{420}\u{416}\u{418}\u{422}", "\u{4fdd}\u{6301}", "\u{6301}\u{6709}"]),
    ("CANCEL",       ["CANCELAR",    "CANCELAR",     "ANNULER",      "ANNULLA",      "ABBRECHEN",    "\u{41e}\u{422}\u{41c}\u{415}\u{41d}\u{410}", "\u{30ad}\u{30e3}\u{30f3}\u{30bb}\u{30eb}", "\u{53d6}\u{6d88}"]),
    ("ADD",          ["A\u{d1}ADIR", "ADICIONAR",    "AJOUTER",      "AGGIUNGI",     "HINZUF\u{dc}GEN", "\u{414}\u{41e}\u{411}\u{410}\u{412}\u{418}\u{422}\u{42c}", "\u{8ffd}\u{52a0}", "\u{6dfb}\u{52a0}"]),
    ("BROWSE",       ["EXAMINAR",    "PROCURAR",     "PARCOURIR",    "SFOGLIA",      "DURCHSUCHEN",  "\u{41e}\u{411}\u{417}\u{41e}\u{420}", "\u{53c2}\u{7167}", "\u{6d4f}\u{89c8}"]),
    ("REMOVE",       ["QUITAR",      "REMOVER",      "SUPPRIMER",    "RIMUOVI",      "ENTFERNEN",    "\u{423}\u{414}\u{410}\u{41b}\u{418}\u{422}\u{42c}", "\u{524a}\u{9664}", "\u{79fb}\u{9664}"]),
    ("DEFAULTS",     ["POR DEFECTO", "PADR\u{d5}ES",  "PAR D\u{c9}FAUT", "PREDEFINITI", "STANDARD",   "\u{41f}\u{41e} \u{423}\u{41c}\u{41e}\u{41b}\u{427}\u{410}\u{41d}\u{418}\u{42e}", "\u{521d}\u{671f}\u{5024}", "\u{9ed8}\u{8ba4}\u{503c}"]),
    ("PLUGIN PATHS", ["RUTAS DE PLUGINS", "CAMINHOS DE PLUGINS", "CHEMINS DES PLUGINS", "PERCORSI PLUGIN", "PLUGIN-PFADE", "\u{41f}\u{423}\u{422}\u{418} \u{41f}\u{41b}\u{410}\u{413}\u{418}\u{41d}\u{41e}\u{412}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{30d1}\u{30b9}", "\u{63d2}\u{4ef6}\u{8def}\u{5f84}"]),
    ("LANGUAGE",     ["IDIOMA",      "IDIOMA",       "LANGUE",       "LINGUA",       "SPRACHE",      "\u{42f}\u{417}\u{42b}\u{41a}", "\u{8a00}\u{8a9e}", "\u{8bed}\u{8a00}"]),
    ("PLAY",         ["TOCAR",       "TOCAR",        "LIRE",         "RIPRODUCI",    "PLAY",         "\u{41f}\u{423}\u{421}\u{41a}", "\u{518d}\u{751f}", "\u{64ad}\u{653e}"]),
    ("STOP",         ["PARAR",       "PARAR",        "STOP",         "STOP",         "STOPP",        "\u{421}\u{422}\u{41e}\u{41f}", "\u{505c}\u{6b62}", "\u{505c}\u{6b62}"]),
    ("STOPPED",      ["DETENIDO",    "PARADO",       "ARR\u{ca}T\u{c9}", "FERMO",     "GESTOPPT",     "\u{41e}\u{421}\u{422}\u{410}\u{41d}\u{41e}\u{412}\u{41b}\u{415}\u{41d}", "\u{505c}\u{6b62}\u{4e2d}", "\u{5df2}\u{505c}\u{6b62}"]),
    ("PLAYING",      ["REPRODUCIENDO", "REPRODUZINDO", "LECTURE",    "IN RIPRODUZIONE", "L\u{c4}UFT", "\u{412}\u{41e}\u{421}\u{41f}\u{420}\u{41e}\u{418}\u{417}\u{412}\u{415}\u{414}\u{415}\u{41d}\u{418}\u{415}", "\u{518d}\u{751f}\u{4e2d}", "\u{64ad}\u{653e}\u{4e2d}"]),
    ("OUT",          ["SALIDA",      "SA\u{cd}DA",    "SORTIE",       "USCITA",       "AUSGANG",      "\u{412}\u{42b}\u{425}\u{41e}\u{414}", "\u{51fa}\u{529b}", "\u{8f93}\u{51fa}"]),
    ("DIRECT",       ["DIRECTO",     "DIRETO",       "DIRECT",       "DIRETTA",      "DIREKT",       "\u{41f}\u{420}\u{42f}\u{41c}\u{41e}\u{419}", "\u{30c0}\u{30a4}\u{30ec}\u{30af}\u{30c8}", "\u{76f4}\u{51fa}"]),
    ("MONO",         ["MONO",        "MONO",         "MONO",         "MONO",         "MONO",         "\u{41c}\u{41e}\u{41d}\u{41e}", "\u{30e2}\u{30ce}", "\u{5355}\u{58f0}\u{9053}"]),
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
    ("ALL BANKS", ["TODOS LOS BANCOS", "TODOS OS BANCOS", "TOUTES LES BANQUES", "TUTTI I BANCHI", "ALLE B\u{c4}NKE", "\u{412}\u{421}\u{415} \u{411}\u{410}\u{41d}\u{41a}\u{418}", "\u{3059}\u{3079}\u{3066}\u{306e}\u{30d0}\u{30f3}\u{30af}", "\u{5168}\u{90e8}\u{97f3}\u{8272}\u{5e93}"]),
    ("OVERWRITE PROJECT?", ["\u{bf}SOBRESCRIBIR PROYECTO?", "SOBRESCREVER PROJETO?", "\u{c9}CRASER LE PROJET\u{a0}?", "SOVRASCRIVERE IL PROGETTO?", "PROJEKT \u{dc}BERSCHREIBEN?", "\u{41f}\u{415}\u{420}\u{415}\u{417}\u{410}\u{41f}\u{418}\u{421}\u{410}\u{422}\u{42c} \u{41f}\u{420}\u{41e}\u{415}\u{41a}\u{422}?", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{4e0a}\u{66f8}\u{304d}\u{ff1f}", "\u{8986}\u{76d6}\u{9879}\u{76ee}\u{ff1f}"]),
    ("OVERWRITE", ["SOBRESCRIBIR", "SOBRESCREVER", "\u{c9}CRASER", "SOVRASCRIVI", "\u{dc}BERSCHREIBEN", "\u{41f}\u{415}\u{420}\u{415}\u{417}\u{410}\u{41f}\u{418}\u{421}\u{410}\u{422}\u{42c}", "\u{4e0a}\u{66f8}\u{304d}", "\u{8986}\u{76d6}"]),
    ("RENAME INSTEAD", ["CAMBIAR EL NOMBRE", "RENOMEAR", "RENOMMER", "RINOMINA", "UMBENENNEN", "\u{41f}\u{415}\u{420}\u{415}\u{418}\u{41c}\u{415}\u{41d}\u{41e}\u{412}\u{410}\u{422}\u{42c}", "\u{540d}\u{524d}\u{3092}\u{5909}\u{3048}\u{308b}", "\u{91cd}\u{547d}\u{540d}"]),
    ("New project", ["Nuevo proyecto", "Novo projeto", "Nouveau projet", "Nuovo progetto", "Neues Projekt", "\u{41d}\u{43e}\u{432}\u{44b}\u{439} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}", "\u{65b0}\u{898f}\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}", "\u{65b0}\u{5efa}\u{9879}\u{76ee}"]),
    ("Save project", ["Guardar proyecto", "Salvar projeto", "Enregistrer le projet", "Salva progetto", "Projekt speichern", "\u{421}\u{43e}\u{445}\u{440}\u{430}\u{43d}\u{438}\u{442}\u{44c} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{4fdd}\u{5b58}", "\u{4fdd}\u{5b58}\u{9879}\u{76ee}"]),
    ("Save project as\u{2026}", ["Guardar proyecto como\u{2026}", "Salvar projeto como\u{2026}", "Enregistrer le projet sous\u{2026}", "Salva progetto come\u{2026}", "Projekt speichern unter\u{2026}", "\u{421}\u{43e}\u{445}\u{440}\u{430}\u{43d}\u{438}\u{442}\u{44c} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442} \u{43a}\u{430}\u{43a}\u{2026}", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{5225}\u{540d}\u{3067}\u{4fdd}\u{5b58}\u{2026}", "\u{9879}\u{76ee}\u{53e6}\u{5b58}\u{4e3a}\u{2026}"]),
    ("Open project\u{2026}", ["Abrir proyecto\u{2026}", "Abrir projeto\u{2026}", "Ouvrir un projet\u{2026}", "Apri progetto\u{2026}", "Projekt \u{f6}ffnen\u{2026}", "\u{41e}\u{442}\u{43a}\u{440}\u{44b}\u{442}\u{44c} \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}\u{2026}", "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{3092}\u{958b}\u{304f}\u{2026}", "\u{6253}\u{5f00}\u{9879}\u{76ee}\u{2026}"]),
    ("Quit",         ["Salir",       "Sair",         "Quitter",      "Esci",         "Beenden",      "\u{412}\u{44b}\u{445}\u{43e}\u{434}", "\u{7d42}\u{4e86}", "\u{9000}\u{51fa}"]),
    ("Settings\u{2026}", ["Ajustes\u{2026}", "Ajustes\u{2026}", "R\u{e9}glages\u{2026}", "Impostazioni\u{2026}", "Einstellungen\u{2026}", "\u{41d}\u{430}\u{441}\u{442}\u{440}\u{43e}\u{439}\u{43a}\u{438}\u{2026}", "\u{8a2d}\u{5b9a}\u{2026}", "\u{8bbe}\u{7f6e}\u{2026}"]),
    ("Rescan plugin paths", ["Reescanear rutas de plugins", "Reexaminar caminhos de plugins", "Ranalyser les chemins de plugins", "Riscansiona percorsi plugin", "Plugin-Pfade neu scannen", "\u{41f}\u{435}\u{440}\u{435}\u{441}\u{43a}\u{430}\u{43d} \u{43f}\u{443}\u{442}\u{435}\u{439} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{43e}\u{432}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{30d1}\u{30b9}\u{3092}\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}", "\u{91cd}\u{65b0}\u{626b}\u{63cf}\u{63d2}\u{4ef6}\u{8def}\u{5f84}"]),
    ("About choz",   ["Acerca de choz", "Sobre o choz", "\u{c0} propos de choz", "Informazioni su choz", "\u{dc}ber choz", "\u{41e} choz", "choz \u{306b}\u{3064}\u{3044}\u{3066}", "\u{5173}\u{4e8e} choz"]),
    ("Rescanning plugin paths", ["Reescaneando rutas de plugins", "Reexaminando caminhos de plugins", "Nouvelle analyse des chemins de plugins", "Nuova scansione dei percorsi plugin", "Plugin-Pfade werden neu gescannt", "\u{41f}\u{415}\u{420}\u{415}\u{421}\u{41a}\u{410}\u{41d} \u{41f}\u{423}\u{422}\u{415}\u{419} \u{41f}\u{41b}\u{410}\u{413}\u{418}\u{41d}\u{41e}\u{412}", "\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}\u{30d1}\u{30b9}\u{3092}\u{518d}\u{30b9}\u{30ad}\u{30e3}\u{30f3}\u{4e2d}", "\u{6b63}\u{5728}\u{91cd}\u{65b0}\u{626b}\u{63cf}\u{63d2}\u{4ef6}\u{8def}\u{5f84}"]),
    // ── What the call sites ask for, in the order the panels reach them ──
    // The rest of the table came first and is grouped by where it is drawn;
    // these are what an audit of every `t("…")` against this list turned up
    // missing — each one had been falling back to English in all eight.
    ("SEQ", ["SEQ", "SEQ", "SEQ", "SEQ", "SEQ", "\u{421}\u{415}\u{41a}\u{412}", "\u{30b7}\u{30fc}\u{30b1}\u{30f3}\u{30b5}\u{30fc}", "\u{97f3}\u{5e8f}\u{5668}"]),
    ("PART", ["PARTE", "PARTE", "PARTIE", "PARTE", "TEIL", "\u{427}\u{410}\u{421}\u{422}\u{42c}", "\u{30d1}\u{30fc}\u{30c8}", "\u{6bb5}\u{843d}"]),
    ("EVENTS", ["EVENTOS", "EVENTOS", "\u{c9}V\u{c9}NEMENTS", "EVENTI", "EREIGNISSE", "\u{421}\u{41e}\u{411}\u{42b}\u{422}\u{418}\u{42f}", "\u{30a4}\u{30d9}\u{30f3}\u{30c8}", "\u{4e8b}\u{4ef6}"]),
    ("QUANTIZE", ["CUANTIZAR", "QUANTIZAR", "QUANTIFIER", "QUANTIZZA", "QUANTISIEREN", "\u{41a}\u{412}\u{410}\u{41d}\u{422}\u{41e}\u{412}\u{410}\u{41d}\u{418}\u{415}", "\u{30af}\u{30aa}\u{30f3}\u{30bf}\u{30a4}\u{30ba}", "\u{91cf}\u{5316}"]),
    ("SPLIT", ["DIVISI\u{d3}N", "DIVIS\u{c3}O", "PARTAGE", "DIVISIONE", "SPLIT", "\u{421}\u{41f}\u{41b}\u{418}\u{422}", "\u{30b9}\u{30d7}\u{30ea}\u{30c3}\u{30c8}", "\u{5206}\u{533a}"]),
    ("GATE", ["PUERTA", "PORTA", "PORTE", "GATE", "GATE", "\u{413}\u{415}\u{419}\u{422}", "\u{30b2}\u{30fc}\u{30c8}", "\u{95e8}\u{9650}"]),
    ("DEPTH", ["PROFUNDIDAD", "PROFUNDIDADE", "PROFONDEUR", "PROFONDIT\u{c0}", "TIEFE", "\u{413}\u{41b}\u{423}\u{411}\u{418}\u{41d}\u{410}", "\u{6df1}\u{3055}", "\u{6df1}\u{5ea6}"]),
    ("THRESHOLD", ["UMBRAL", "LIMIAR", "SEUIL", "SOGLIA", "SCHWELLE", "\u{41f}\u{41e}\u{420}\u{41e}\u{413}", "\u{3057}\u{304d}\u{3044}\u{5024}", "\u{9608}\u{503c}"]),
    ("RELEASE", ["CA\u{cd}DA", "LIBERA\u{c7}\u{c3}O", "REL\u{c2}CHEMENT", "RILASCIO", "RELEASE", "\u{421}\u{41f}\u{410}\u{414}", "\u{30ea}\u{30ea}\u{30fc}\u{30b9}", "\u{91ca}\u{653e}"]),
    ("MODE", ["MODO", "MODO", "MODE", "MODO", "MODUS", "\u{420}\u{415}\u{416}\u{418}\u{41c}", "\u{30e2}\u{30fc}\u{30c9}", "\u{6a21}\u{5f0f}"]),
    ("CLOCK", ["RELOJ", "REL\u{d3}GIO", "HORLOGE", "CLOCK", "TAKT", "\u{427}\u{410}\u{421}\u{42b}", "\u{30af}\u{30ed}\u{30c3}\u{30af}", "\u{65f6}\u{949f}"]),
    ("INTERNAL", ["INTERNO", "INTERNO", "INTERNE", "INTERNO", "INTERN", "\u{412}\u{41d}\u{423}\u{422}\u{420}\u{415}\u{41d}\u{41d}\u{418}\u{419}", "\u{5185}\u{90e8}", "\u{5185}\u{90e8}"]),
    ("ANY PORT", ["CUALQUIER PUERTO", "QUALQUER PORTA", "TOUT PORT", "QUALSIASI PORTA", "BELIEBIGER PORT", "\u{41b}\u{42e}\u{411}\u{41e}\u{419} \u{41f}\u{41e}\u{420}\u{422}", "\u{4efb}\u{610f}\u{306e}\u{30dd}\u{30fc}\u{30c8}", "\u{4efb}\u{610f}\u{7aef}\u{53e3}"]),
    ("PATCH A DAW HERE", ["CONECTA AQU\u{cd} UN DAW", "CONECTE UM DAW AQUI", "BRANCHEZ UN DAW ICI", "COLLEGA QUI UN DAW", "HIER EINE DAW ANSCHLIESSEN", "\u{41f}\u{41e}\u{414}\u{41a}\u{41b}\u{42e}\u{427}\u{418}\u{422}\u{415} DAW", "DAW\u{3092}\u{3053}\u{3053}\u{306b}\u{63a5}\u{7d9a}", "\u{5728}\u{6b64}\u{8fde}\u{63a5} DAW"]),
    ("TEMPO", ["TEMPO", "ANDAMENTO", "TEMPO", "TEMPO", "TEMPO", "\u{422}\u{415}\u{41c}\u{41f}", "\u{30c6}\u{30f3}\u{30dd}", "\u{901f}\u{5ea6}"]),
    ("TIME SIGNATURE", ["COMP\u{c1}S", "F\u{d3}RMULA DE COMPASSO", "M\u{c9}TRIQUE", "METRO", "TAKTART", "\u{420}\u{410}\u{417}\u{41c}\u{415}\u{420}", "\u{62cd}\u{5b50}", "\u{62cd}\u{53f7}"]),
    ("STEPS", ["PASOS", "PASSOS", "PAS", "PASSI", "SCHRITTE", "\u{428}\u{410}\u{413}\u{418}", "\u{30b9}\u{30c6}\u{30c3}\u{30d7}", "\u{6b65}"]),
    ("BEATS", ["PULSOS", "TEMPOS", "TEMPS", "MOVIMENTI", "SCHL\u{c4}GE", "\u{414}\u{41e}\u{41b}\u{418}", "\u{62cd}", "\u{62cd}"]),
    ("GROUPING", ["AGRUPACI\u{d3}N", "AGRUPAMENTO", "GROUPEMENT", "RAGGRUPPAMENTO", "GRUPPIERUNG", "\u{413}\u{420}\u{423}\u{41f}\u{41f}\u{418}\u{420}\u{41e}\u{412}\u{41a}\u{410}", "\u{30b0}\u{30eb}\u{30fc}\u{30d7}\u{5206}\u{3051}", "\u{5206}\u{7ec4}"]),
    ("CLICK", ["CLIC", "CLIQUE", "CLIC", "CLICK", "KLICK", "\u{429}\u{415}\u{41b}\u{427}\u{41e}\u{41a}", "\u{30af}\u{30ea}\u{30c3}\u{30af}", "\u{5494}\u{55d2}\u{58f0}"]),
    ("METRONOME", ["METR\u{d3}NOMO", "METR\u{d4}NOMO", "M\u{c9}TRONOME", "METRONOMO", "METRONOM", "\u{41c}\u{415}\u{422}\u{420}\u{41e}\u{41d}\u{41e}\u{41c}", "\u{30e1}\u{30c8}\u{30ed}\u{30ce}\u{30fc}\u{30e0}", "\u{8282}\u{62cd}\u{5668}"]),
    ("MET", ["MET", "MET", "M\u{c9}T", "MET", "MET", "\u{41c}\u{415}\u{422}", "\u{30e1}\u{30c8}", "\u{8282}\u{62cd}"]),
    ("OUTPUT", ["SALIDA", "SA\u{cd}DA", "SORTIE", "USCITA", "AUSGANG", "\u{412}\u{42b}\u{425}\u{41e}\u{414}", "\u{51fa}\u{529b}", "\u{8f93}\u{51fa}"]),
    ("HARMONICS", ["ARM\u{d3}NICOS", "HARM\u{d4}NICOS", "HARMONIQUES", "ARMONICHE", "OBERT\u{d6}NE", "\u{413}\u{410}\u{420}\u{41c}\u{41e}\u{41d}\u{418}\u{41a}\u{418}", "\u{500d}\u{97f3}", "\u{6cdb}\u{97f3}"]),
    ("HARMONIC", ["ARM\u{d3}NICO", "HARM\u{d4}NICO", "HARMONIQUE", "ARMONICA", "OBERTON", "\u{413}\u{410}\u{420}\u{41c}\u{41e}\u{41d}\u{418}\u{41a}\u{410}", "\u{500d}\u{97f3}", "\u{6cdb}\u{97f3}"]),
    ("MAG", ["MAG", "MAG", "AMPL", "MAG", "BETRAG", "\u{410}\u{41c}\u{41f}\u{41b}", "\u{632f}\u{5e45}", "\u{5e45}\u{5ea6}"]),
    ("PHASE", ["FASE", "FASE", "PHASE", "FASE", "PHASE", "\u{424}\u{410}\u{417}\u{410}", "\u{4f4d}\u{76f8}", "\u{76f8}\u{4f4d}"]),
    ("SILENT", ["SILENCIO", "SIL\u{ca}NCIO", "SILENCE", "SILENZIO", "STUMM", "\u{422}\u{418}\u{428}\u{418}\u{41d}\u{410}", "\u{7121}\u{97f3}", "\u{9759}\u{97f3}"]),
    ("FULL", ["LLENO", "CHEIO", "PLEIN", "PIENO", "VOLL", "\u{41f}\u{41e}\u{41b}\u{41d}\u{42b}\u{419}", "\u{6700}\u{5927}", "\u{6ee1}"]),
    ("CLOSE", ["CERRAR", "FECHAR", "FERMER", "CHIUDI", "SCHLIESSEN", "\u{417}\u{410}\u{41a}\u{420}\u{42b}\u{422}\u{42c}", "\u{9589}\u{3058}\u{308b}", "\u{5173}\u{95ed}"]),
    ("SOUND", ["SONIDO", "SOM", "SON", "SUONO", "KLANG", "\u{417}\u{412}\u{423}\u{41a}", "\u{97f3}\u{8272}", "\u{97f3}\u{8272}"]),
    ("SOUNDS", ["SONIDOS", "SONS", "SONS", "SUONI", "KL\u{c4}NGE", "\u{417}\u{412}\u{423}\u{41a}\u{418}", "\u{97f3}\u{8272}", "\u{97f3}\u{8272}"]),
    ("CHORD", ["ACORDE", "ACORDE", "ACCORD", "ACCORDO", "AKKORD", "\u{410}\u{41a}\u{41a}\u{41e}\u{420}\u{414}", "\u{30b3}\u{30fc}\u{30c9}", "\u{548c}\u{5f26}"]),
    ("CHORD INPUT", ["ENTRADA DE ACORDE", "ENTRADA DE ACORDE", "ENTR\u{c9}E D'ACCORD", "INGRESSO ACCORDO", "AKKORD-EINGANG", "\u{412}\u{425}\u{41e}\u{414} \u{410}\u{41a}\u{41a}\u{41e}\u{420}\u{414}\u{410}", "\u{30b3}\u{30fc}\u{30c9}\u{5165}\u{529b}", "\u{548c}\u{5f26}\u{8f93}\u{5165}"]),
    ("ANY", ["CUALQUIERA", "QUALQUER", "TOUT", "QUALSIASI", "BELIEBIG", "\u{41b}\u{42e}\u{411}\u{41e}\u{419}", "\u{3059}\u{3079}\u{3066}", "\u{4efb}\u{610f}"]),
    ("ANY KEYBOARD", ["CUALQUIER TECLADO", "QUALQUER TECLADO", "TOUT CLAVIER", "QUALSIASI TASTIERA", "BELIEBIGE TASTATUR", "\u{41b}\u{42e}\u{411}\u{410}\u{42f} \u{41a}\u{41b}\u{410}\u{412}\u{418}\u{410}\u{422}\u{423}\u{420}\u{410}", "\u{4efb}\u{610f}\u{306e}\u{30ad}\u{30fc}\u{30dc}\u{30fc}\u{30c9}", "\u{4efb}\u{610f}\u{952e}\u{76d8}"]),
    ("QWERTY", ["QWERTY", "QWERTY", "AZERTY", "QWERTY", "QWERTZ", "QWERTY", "QWERTY", "QWERTY"]),
    ("LEVEL", ["NIVEL", "N\u{cd}VEL", "NIVEAU", "LIVELLO", "PEGEL", "\u{423}\u{420}\u{41e}\u{412}\u{415}\u{41d}\u{42c}", "\u{30ec}\u{30d9}\u{30eb}", "\u{7535}\u{5e73}"]),
    ("UNIT", ["UNIDAD", "UNIDADE", "UNIT\u{c9}", "UNIT\u{c0}", "EINHEIT", "\u{415}\u{414}\u{418}\u{41d}\u{418}\u{426}\u{410}", "\u{5358}\u{4f4d}", "\u{5355}\u{4f4d}"]),
    ("COLOUR", ["COLOR", "COR", "COULEUR", "COLORE", "FARBE", "\u{426}\u{412}\u{415}\u{422}", "\u{8272}", "\u{989c}\u{8272}"]),
    ("OFF", ["APAGADO", "DESLIGADO", "ARR\u{ca}T", "SPENTO", "AUS", "\u{412}\u{42b}\u{41a}\u{41b}", "\u{30aa}\u{30d5}", "\u{5173}\u{95ed}"]),
    ("empty", ["vac\u{ed}o", "vazio", "vide", "vuoto", "leer", "\u{43f}\u{443}\u{441}\u{442}\u{43e}", "\u{7a7a}", "\u{7a7a}"]),
    ("Loading", ["Cargando", "Carregando", "Chargement", "Caricamento", "Wird geladen", "\u{417}\u{430}\u{433}\u{440}\u{443}\u{437}\u{43a}\u{430}", "\u{8aad}\u{307f}\u{8fbc}\u{307f}\u{4e2d}", "\u{52a0}\u{8f7d}\u{4e2d}"]),
    ("NO PREVIEW", ["SIN VISTA PREVIA", "SEM PR\u{c9}VIA", "AUCUN APER\u{c7}U", "NESSUNA ANTEPRIMA", "KEINE VORSCHAU", "\u{41d}\u{415}\u{422} \u{41f}\u{420}\u{415}\u{414}\u{41f}\u{420}\u{41e}\u{421}\u{41c}\u{41e}\u{422}\u{420}\u{410}", "\u{30d7}\u{30ec}\u{30d3}\u{30e5}\u{30fc}\u{306a}\u{3057}", "\u{65e0}\u{9884}\u{89c8}"]),
    ("WHAT IS PLAYING NOW", ["LO QUE SUENA AHORA", "O QUE EST\u{c1} TOCANDO", "CE QUI JOUE", "CI\u{d2} CHE SUONA ORA", "WAS GERADE KLINGT", "\u{427}\u{422}\u{41e} \u{417}\u{412}\u{423}\u{427}\u{418}\u{422} \u{421}\u{415}\u{419}\u{427}\u{410}\u{421}", "\u{73fe}\u{5728}\u{306e}\u{97f3}", "\u{5f53}\u{524d}\u{97f3}\u{8272}"]),
    ("  click an octave to give it the chosen sound, right-click to clear it", ["  clic en una octava para darle el sonido elegido, clic derecho para quitarlo", "  clique numa oitava para dar-lhe o som escolhido, bot\u{e3}o direito limpa", "  cliquez sur une octave pour lui donner le son choisi, clic droit efface", "  clicca un'ottava per darle il suono scelto, tasto destro per toglierlo", "  eine Oktave anklicken, um ihr den Klang zu geben; Rechtsklick l\u{f6}scht ihn", "  \u{449}\u{451}\u{43b}\u{43a}\u{43d}\u{438}\u{442}\u{435} \u{43e}\u{43a}\u{442}\u{430}\u{432}\u{443}, \u{447}\u{442}\u{43e}\u{431}\u{44b} \u{434}\u{430}\u{442}\u{44c} \u{435}\u{439} \u{432}\u{44b}\u{431}\u{440}\u{430}\u{43d}\u{43d}\u{44b}\u{439} \u{437}\u{432}\u{443}\u{43a}; \u{43f}\u{440}\u{430}\u{432}\u{430}\u{44f} \u{43a}\u{43d}\u{43e}\u{43f}\u{43a}\u{430} \u{443}\u{431}\u{438}\u{440}\u{430}\u{435}\u{442}", "  \u{30aa}\u{30af}\u{30bf}\u{30fc}\u{30d6}\u{3092}\u{30af}\u{30ea}\u{30c3}\u{30af}\u{3057}\u{3066}\u{9078}\u{3093}\u{3060}\u{97f3}\u{8272}\u{3092}\u{5272}\u{308a}\u{5f53}\u{3066}\u{3001}\u{53f3}\u{30af}\u{30ea}\u{30c3}\u{30af}\u{3067}\u{89e3}\u{9664}", "  \u{70b9}\u{51fb}\u{516b}\u{5ea6}\u{4ee5}\u{6307}\u{5b9a}\u{6240}\u{9009}\u{97f3}\u{8272}\u{ff0c}\u{53f3}\u{952e}\u{6e05}\u{9664}"]),
    ("  click a key to give this lane its note \u{2014} it sounds as you pick", ["  clic en una tecla para dar su nota a esta pista \u{2014} suena al elegirla", "  clique numa tecla para dar a nota desta faixa \u{2014} soa ao escolher", "  cliquez sur une touche pour donner sa note \u{e0} cette piste \u{2014} elle sonne", "  clicca un tasto per dare la nota a questa traccia \u{2014} suona mentre scegli", "  eine Taste anklicken, um dieser Spur ihre Note zu geben \u{2014} sie klingt dabei", "  \u{449}\u{451}\u{43b}\u{43a}\u{43d}\u{438}\u{442}\u{435} \u{43a}\u{43b}\u{430}\u{432}\u{438}\u{448}\u{443}, \u{447}\u{442}\u{43e}\u{431}\u{44b} \u{437}\u{430}\u{434}\u{430}\u{442}\u{44c} \u{43d}\u{43e}\u{442}\u{443} \u{434}\u{43e}\u{440}\u{43e}\u{436}\u{43a}\u{438} \u{2014} \u{43e}\u{43d}\u{430} \u{437}\u{432}\u{443}\u{447}\u{438}\u{442} \u{43f}\u{440}\u{438} \u{432}\u{44b}\u{431}\u{43e}\u{440}\u{435}", "  \u{9375}\u{76e4}\u{3092}\u{30af}\u{30ea}\u{30c3}\u{30af}\u{3057}\u{3066}\u{3053}\u{306e}\u{30ec}\u{30fc}\u{30f3}\u{306e}\u{97f3}\u{3092}\u{6c7a}\u{3081}\u{308b} \u{2014} \u{9078}\u{3076}\u{3068}\u{9cf4}\u{308b}", "  \u{70b9}\u{51fb}\u{7434}\u{952e}\u{4e3a}\u{8be5}\u{97f3}\u{8f68}\u{6307}\u{5b9a}\u{97f3}\u{7b26} \u{2014} \u{9009}\u{62e9}\u{65f6}\u{4f1a}\u{53d1}\u{58f0}"]),
    ("play the highlighted patch on the keyboard \u{00B7} SELECT keeps it", ["toca el patch resaltado en el teclado \u{00B7} ELEGIR lo conserva", "toque o patch destacado no teclado \u{00B7} SELECIONAR o mant\u{e9}m", "jouez le patch surlign\u{e9} au clavier \u{00B7} CHOISIR le conserve", "suona il patch evidenziato sulla tastiera \u{00B7} SCEGLI lo mantiene", "das markierte Patch auf der Tastatur spielen \u{00B7} W\u{c4}HLEN beh\u{e4}lt es", "\u{441}\u{44b}\u{433}\u{440}\u{430}\u{439}\u{442}\u{435} \u{432}\u{44b}\u{434}\u{435}\u{43b}\u{435}\u{43d}\u{43d}\u{44b}\u{439} \u{43f}\u{430}\u{442}\u{447} \u{43d}\u{430} \u{43a}\u{43b}\u{430}\u{432}\u{438}\u{430}\u{442}\u{443}\u{440}\u{435} \u{00B7} \u{412}\u{42b}\u{411}\u{420}\u{410}\u{422}\u{42c} \u{43e}\u{441}\u{442}\u{430}\u{432}\u{438}\u{442} \u{435}\u{433}\u{43e}", "\u{5f37}\u{8abf}\u{3055}\u{308c}\u{305f}\u{97f3}\u{8272}\u{3092}\u{9375}\u{76e4}\u{3067}\u{8a66}\u{3059} \u{00B7} \u{9078}\u{629e}\u{3067}\u{78ba}\u{5b9a}", "\u{5728}\u{952e}\u{76d8}\u{4e0a}\u{8bd5}\u{542c}\u{9ad8}\u{4eae}\u{97f3}\u{8272} \u{00B7} \u{9009}\u{62e9}\u{5373}\u{4fdd}\u{7559}"]),
    // ── The looper deck ──────────────────────────────────────────────────
    ("EMPTY", ["VAC\u{cd}O", "VAZIO", "VIDE", "VUOTO", "LEER", "\u{41f}\u{423}\u{421}\u{422}\u{41e}", "\u{7a7a}", "\u{7a7a}"]),
    ("EXPORT", ["EXPORTAR", "EXPORTAR", "EXPORTER", "ESPORTA", "EXPORTIEREN", "\u{42d}\u{41a}\u{421}\u{41f}\u{41e}\u{420}\u{422}", "\u{66f8}\u{304d}\u{51fa}\u{3057}", "\u{5bfc}\u{51fa}"]),
    ("CHANNEL", ["CANAL", "CANAL", "CANAL", "CANALE", "KANAL", "\u{41a}\u{410}\u{41d}\u{410}\u{41b}", "\u{30c1}\u{30e3}\u{30f3}\u{30cd}\u{30eb}", "\u{901a}\u{9053}"]),
    ("REC", ["GRAB", "GRAV", "ENR", "REG", "AUFN", "\u{417}\u{410}\u{41f}", "\u{9332}\u{97f3}", "\u{5f55}\u{97f3}"]),
    ("PAUSE", ["PAUSA", "PAUSA", "PAUSE", "PAUSA", "PAUSE", "\u{41f}\u{410}\u{423}\u{417}\u{410}", "\u{4e00}\u{6642}\u{505c}\u{6b62}", "\u{6682}\u{505c}"]),
    ("CLEAR", ["BORRAR", "LIMPAR", "EFFACER", "CANCELLA", "L\u{d6}SCHEN", "\u{421}\u{422}\u{415}\u{420}\u{415}\u{422}\u{42c}", "\u{6d88}\u{53bb}", "\u{6e05}\u{9664}"]),
    ("EXPORT LOOPS", ["EXPORTAR LOOPS", "EXPORTAR LOOPS", "EXPORTER LES BOUCLES", "ESPORTA I LOOP", "LOOPS EXPORTIEREN", "\u{42d}\u{41a}\u{421}\u{41f}\u{41e}\u{420}\u{422} \u{41b}\u{423}\u{41f}\u{41e}\u{412}", "\u{30eb}\u{30fc}\u{30d7}\u{3092}\u{66f8}\u{304d}\u{51fa}\u{3059}", "\u{5bfc}\u{51fa}\u{5faa}\u{73af}"]),
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

    /// Every `t("…")` in the interface has a row here, and every row is asked
    /// for by something.
    ///
    /// The table's own tests only ever checked it against itself, so the one
    /// mistake that mattered went unseen from both sides: a call site with no
    /// row falls back to English **in all eight languages** and looks like a
    /// string somebody chose to leave alone, and a row nothing asks for is
    /// eight translations of a label that is not on screen. An audit turned up
    /// forty-six of the first and three of the second.
    ///
    /// Read off the source, because that is where the truth is. Keys reached
    /// through a variable — the menu's rows, the RACK's button labels — are
    /// matched as plain literals, which is why the second half looks for the
    /// string rather than for the call.
    #[test]
    fn the_table_and_the_call_sites_are_the_same_list() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the crate has a src/") {
                let path = entry.expect("readable").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|n| n != "i18n.rs")
                {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    sources.push((name, std::fs::read_to_string(&path).expect("readable")));
                }
            }
        }
        assert!(sources.len() > 5, "found no sources to read");

        // `t("…")`, taking the literal between the quotes exactly as written —
        // escapes included, because the table writes them the same way.
        let calls = |text: &str| -> Vec<String> {
            let mut out = Vec::new();
            let bytes: Vec<char> = text.chars().collect();
            let open: Vec<char> = "t(\"".chars().collect();
            let mut i = 0;
            while i + open.len() < bytes.len() {
                let starts = bytes[i..i + open.len()] == open[..]
                    // `format!("…")` and `next("…")` are not `t("…")`.
                    && (i == 0 || !bytes[i - 1].is_alphanumeric() && bytes[i - 1] != '_');
                if !starts {
                    i += 1;
                    continue;
                }
                let mut j = i + open.len();
                let mut key = String::new();
                while j < bytes.len() && bytes[j] != '"' {
                    if bytes[j] == '\\' && j + 1 < bytes.len() {
                        key.push(bytes[j]);
                        j += 1;
                    }
                    key.push(bytes[j]);
                    j += 1;
                }
                if j + 1 < bytes.len() && bytes[j] == '"' && bytes[j + 1] == ')' {
                    out.push(key);
                }
                i = j.max(i + 1);
            }
            out
        };

        let rows: Vec<&str> = TABLE.iter().map(|(k, _)| *k).collect();
        let mut untranslated: Vec<String> = Vec::new();
        for (name, text) in sources.iter() {
            for key in calls(text) {
                // The table holds the key unescaped; the source writes it with
                // escapes. Compare on what Rust will have made of both.
                if !rows.iter().any(|r| same(r, &key)) {
                    untranslated.push(format!("{name}: {key:?}"));
                }
            }
        }
        untranslated.sort();
        untranslated.dedup();
        assert!(
            untranslated.is_empty(),
            "these fall back to English in all eight languages:\n  {}",
            untranslated.join("\n  ")
        );

        let unread: Vec<&str> = rows
            .iter()
            .copied()
            .filter(|row| {
                // Written either way in the source: plain, or with the
                // non-ASCII spelled `\u{2026}` the way this file does it.
                let escaped: String = row
                    .chars()
                    .map(|c| match c.is_ascii() {
                        true => c.to_string(),
                        false => format!("\\u{{{:x}}}", c as u32),
                    })
                    .collect();
                !sources.iter().any(|(_, text)| {
                    calls(text).iter().any(|k| same(row, k))
                        || text.contains(&format!("\"{row}\""))
                        || text.contains(&format!("\"{escaped}\""))
                })
            })
            .collect();
        assert!(
            unread.is_empty(),
            "nothing on screen asks for these rows: {unread:?}"
        );
    }

    /// Whether a row's key and a source literal are the same string once Rust
    /// has read the escapes in the second.
    #[cfg(test)]
    fn same(row: &str, literal: &str) -> bool {
        let mut out = String::new();
        let mut chars = literal.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('u') => {
                    // `\u{2014}`
                    let _ = chars.next(); // '{'
                    let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(c) => out.push(c),
                        None => return false,
                    }
                }
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => return false,
            }
        }
        out == row
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

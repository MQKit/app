//! The few words the tools write into their results (a page heading in OCR text, a mail's
//! header, a stamp, sheet and file names), in each of the interface's languages, chosen
//! by the job's locale ("de-DE" → German). English when the language isn't one of them.

pub struct Words {
    /// "Page {n}"
    pub page_n: &'static str,
    /// "Page {n} of {total}"
    pub page_of: &'static str,
    pub confidential: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub cc: &'static str,
    pub date: &'static str,
    /// The attachments' heading in a printed mail.
    pub attachments: &'static str,
    /// The attachments' folder name: "<message> - attachments".
    pub attachments_folder: &'static str,
    /// The file name of several messages printed together.
    pub messages: &'static str,
    /// "Table {n}", a sheet's name.
    pub table_n: &'static str,
    /// One sheet holding every table.
    pub tables: &'static str,
    pub odd: &'static str,
    pub even: &'static str,
    /// A mail's date (chrono format).
    pub date_format: &'static str,
}

impl Words {
    pub fn page(&self, n: impl std::fmt::Display) -> String {
        self.page_n.replace("{n}", &n.to_string())
    }
    pub fn page_of(&self, n: impl std::fmt::Display, total: impl std::fmt::Display) -> String {
        self.page_of.replace("{n}", &n.to_string()).replace("{total}", &total.to_string())
    }
    pub fn table(&self, n: impl std::fmt::Display) -> String {
        self.table_n.replace("{n}", &n.to_string())
    }
}

const EN: Words = Words { page_n: "Page {n}", page_of: "Page {n} of {total}", confidential: "CONFIDENTIAL", from: "From", to: "To", cc: "Cc", date: "Date", attachments: "Attachments", attachments_folder: "attachments", messages: "messages", table_n: "Table {n}", tables: "Tables", odd: "odd", even: "even", date_format: "%Y-%m-%d %H:%M" };
const RO: Words = Words { page_n: "Pagina {n}", page_of: "Pagina {n} din {total}", confidential: "CONFIDENȚIAL", from: "De la", to: "Către", cc: "Cc", date: "Data", attachments: "Atașamente", attachments_folder: "atașamente", messages: "mesaje", table_n: "Tabel {n}", tables: "Tabele", odd: "impare", even: "pare", date_format: "%d.%m.%Y %H:%M" };
const DE: Words = Words { page_n: "Seite {n}", page_of: "Seite {n} von {total}", confidential: "VERTRAULICH", from: "Von", to: "An", cc: "Cc", date: "Datum", attachments: "Anhänge", attachments_folder: "Anhänge", messages: "Nachrichten", table_n: "Tabelle {n}", tables: "Tabellen", odd: "ungerade", even: "gerade", date_format: "%d.%m.%Y %H:%M" };
const ES: Words = Words { page_n: "Página {n}", page_of: "Página {n} de {total}", confidential: "CONFIDENCIAL", from: "De", to: "Para", cc: "CC", date: "Fecha", attachments: "Adjuntos", attachments_folder: "adjuntos", messages: "mensajes", table_n: "Tabla {n}", tables: "Tablas", odd: "impares", even: "pares", date_format: "%d/%m/%Y %H:%M" };
const FR: Words = Words { page_n: "Page {n}", page_of: "Page {n} sur {total}", confidential: "CONFIDENTIEL", from: "De", to: "À", cc: "Cc", date: "Date", attachments: "Pièces jointes", attachments_folder: "pièces jointes", messages: "messages", table_n: "Tableau {n}", tables: "Tableaux", odd: "impaires", even: "paires", date_format: "%d/%m/%Y %H:%M" };
const IT: Words = Words { page_n: "Pagina {n}", page_of: "Pagina {n} di {total}", confidential: "RISERVATO", from: "Da", to: "A", cc: "Cc", date: "Data", attachments: "Allegati", attachments_folder: "allegati", messages: "messaggi", table_n: "Tabella {n}", tables: "Tabelle", odd: "dispari", even: "pari", date_format: "%d/%m/%Y %H:%M" };
const PT: Words = Words { page_n: "Página {n}", page_of: "Página {n} de {total}", confidential: "CONFIDENCIAL", from: "De", to: "Para", cc: "Cc", date: "Data", attachments: "Anexos", attachments_folder: "anexos", messages: "mensagens", table_n: "Tabela {n}", tables: "Tabelas", odd: "ímpares", even: "pares", date_format: "%d/%m/%Y %H:%M" };
const PL: Words = Words { page_n: "Strona {n}", page_of: "Strona {n} z {total}", confidential: "POUFNE", from: "Od", to: "Do", cc: "DW", date: "Data", attachments: "Załączniki", attachments_folder: "załączniki", messages: "wiadomości", table_n: "Tabela {n}", tables: "Tabele", odd: "nieparzyste", even: "parzyste", date_format: "%d.%m.%Y %H:%M" };
const TR: Words = Words { page_n: "Sayfa {n}", page_of: "Sayfa {n} / {total}", confidential: "GİZLİ", from: "Kimden", to: "Kime", cc: "Bilgi", date: "Tarih", attachments: "Ekler", attachments_folder: "ekler", messages: "mesajlar", table_n: "Tablo {n}", tables: "Tablolar", odd: "tek", even: "çift", date_format: "%d.%m.%Y %H:%M" };
const RU: Words = Words { page_n: "Страница {n}", page_of: "Страница {n} из {total}", confidential: "КОНФИДЕНЦИАЛЬНО", from: "От", to: "Кому", cc: "Копия", date: "Дата", attachments: "Вложения", attachments_folder: "вложения", messages: "сообщения", table_n: "Таблица {n}", tables: "Таблицы", odd: "нечётные", even: "чётные", date_format: "%d.%m.%Y %H:%M" };
const JA: Words = Words { page_n: "{n} ページ", page_of: "{n} / {total} ページ", confidential: "機密", from: "差出人", to: "宛先", cc: "CC", date: "日付", attachments: "添付ファイル", attachments_folder: "添付ファイル", messages: "メッセージ", table_n: "表 {n}", tables: "表", odd: "奇数ページ", even: "偶数ページ", date_format: "%Y/%m/%d %H:%M" };
const ZH: Words = Words { page_n: "第 {n} 页", page_of: "第 {n} 页，共 {total} 页", confidential: "机密", from: "发件人", to: "收件人", cc: "抄送", date: "日期", attachments: "附件", attachments_folder: "附件", messages: "邮件", table_n: "表 {n}", tables: "表格", odd: "奇数页", even: "偶数页", date_format: "%Y/%m/%d %H:%M" };

/// The words for a job's locale ("ro-RO", "ja", None…).
pub fn of(locale: Option<&str>) -> &'static Words {
    match locale.unwrap_or("en").split(['-', '_']).next().unwrap_or("en").to_ascii_lowercase().as_str() {
        "ro" => &RO,
        "de" => &DE,
        "es" => &ES,
        "fr" => &FR,
        "it" => &IT,
        "pt" => &PT,
        "pl" => &PL,
        "tr" => &TR,
        "ru" => &RU,
        "ja" => &JA,
        "zh" => &ZH,
        _ => &EN,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn by_locale() {
        assert_eq!(super::of(Some("ro-RO")).page_of(3, 9), "Pagina 3 din 9");
        assert_eq!(super::of(Some("de-DE")).page(2), "Seite 2");
        assert_eq!(super::of(Some("zh-CN")).page(5), "第 5 页");
        assert_eq!(super::of(Some("xx")).table(1), "Table 1");
        assert_eq!(super::of(None).confidential, "CONFIDENTIAL");
    }
}

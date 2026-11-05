//! A reader for the `INSERT ... VALUES` statements Civ5 mods use for text and atlas rows.
//! Everything that is not an INSERT is skipped.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Val {
    Str(String),
    /// A number or a bare word as written (`64`, `-1`, `NULL`, `true`).
    Raw(String),
}

impl Val {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) | Val::Raw(s) => Some(s),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insert {
    pub table: String,
    /// Empty when the statement has no column list.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Val>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Str(String),
    Punct(char),
}

fn tokenize(sql: &str) -> Vec<Tok> {
    let ch: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        if c.is_whitespace() || c == '\u{feff}' {
            i += 1;
        } else if c == '-' && ch.get(i + 1) == Some(&'-') {
            while i < ch.len() && ch[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && ch.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < ch.len() && !(ch[i] == '*' && ch[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '\'' {
            let mut s = String::new();
            i += 1;
            while i < ch.len() {
                if ch[i] == '\'' {
                    if ch.get(i + 1) == Some(&'\'') {
                        s.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                s.push(ch[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if c == '"' || c == '[' || c == '`' {
            let close = if c == '[' { ']' } else { c };
            let mut s = String::new();
            i += 1;
            while i < ch.len() && ch[i] != close {
                s.push(ch[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Word(s));
        } else if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '+' {
            let mut s = String::new();
            while i < ch.len()
                && (ch[i].is_alphanumeric() || matches!(ch[i], '_' | '-' | '.' | '+'))
            {
                s.push(ch[i]);
                i += 1;
            }
            out.push(Tok::Word(s));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

fn is_word(t: Option<&Tok>, w: &str) -> bool {
    matches!(t, Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
}

/// Every `INSERT` statement of `sql`.
pub fn parse_inserts(sql: &str) -> Vec<Insert> {
    let toks = tokenize(sql);
    let mut out = Vec::new();
    for stmt in toks.split(|t| *t == Tok::Punct(';')) {
        if let Some(ins) = parse_stmt(stmt) {
            out.push(ins);
        }
    }
    out
}

fn parse_stmt(t: &[Tok]) -> Option<Insert> {
    let mut i = 0;
    if !is_word(t.get(i), "insert") && !is_word(t.get(i), "replace") {
        return None;
    }
    i += 1;
    if is_word(t.get(i), "or") {
        i += 2; // OR REPLACE / OR IGNORE
    }
    if !is_word(t.get(i), "into") {
        return None;
    }
    i += 1;
    let Some(Tok::Word(table)) = t.get(i) else {
        return None;
    };
    let table = table.clone();
    i += 1;
    let mut columns = Vec::new();
    if t.get(i) == Some(&Tok::Punct('(')) {
        i += 1;
        while i < t.len() && t[i] != Tok::Punct(')') {
            if let Tok::Word(w) = &t[i] {
                columns.push(w.clone());
            }
            i += 1;
        }
        i += 1;
    }
    if !is_word(t.get(i), "values") {
        return None;
    }
    i += 1;
    let mut rows = Vec::new();
    while t.get(i) == Some(&Tok::Punct('(')) {
        i += 1;
        let mut row = Vec::new();
        while i < t.len() && t[i] != Tok::Punct(')') {
            match &t[i] {
                Tok::Str(s) => row.push(Val::Str(s.clone())),
                Tok::Word(w) => row.push(Val::Raw(w.clone())),
                Tok::Punct(',') => {}
                Tok::Punct(_) => {}
            }
            i += 1;
        }
        i += 1;
        rows.push(row);
        if t.get(i) == Some(&Tok::Punct(',')) {
            i += 1;
        }
    }
    Some(Insert {
        table,
        columns,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rows_with_comments_and_escapes() {
        let sql = "INSERT OR REPLACE INTO Language_en_US (Tag, Text)\nVALUES\n-- note; with a semicolon\n\t('TXT_A', 'It''s; fine'),\n\t('TXT_B', 'two\nlines'); /* c */ DELETE FROM X;";
        let v = parse_inserts(sql);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].table, "Language_en_US");
        assert_eq!(v[0].columns, ["Tag", "Text"]);
        assert_eq!(v[0].rows.len(), 2);
        assert_eq!(v[0].rows[0][1], Val::Str("It's; fine".into()));
        assert_eq!(v[0].rows[1][1], Val::Str("two\nlines".into()));
    }

    #[test]
    fn numbers_null_and_quoted_names() {
        let v = parse_inserts(
            "insert into \"IconTextureAtlases\" (Atlas,IconSize,Filename,IconsPerRow,IconsPerColumn) values ('A',64,'a.dds',8,-1),('B',NULL,'b',1,1);",
        );
        assert_eq!(v[0].table, "IconTextureAtlases");
        assert_eq!(v[0].rows[0][1], Val::Raw("64".into()));
        assert_eq!(v[0].rows[0][4], Val::Raw("-1".into()));
        assert_eq!(v[0].rows[1][1], Val::Raw("NULL".into()));
    }

    #[test]
    fn statements_that_are_not_inserts_are_skipped() {
        assert!(parse_inserts("UPDATE X SET a = 1; CREATE TABLE t (a);").is_empty());
    }

    #[test]
    fn no_column_list() {
        let v = parse_inserts("INSERT INTO T VALUES ('a', 2);");
        assert!(v[0].columns.is_empty());
        assert_eq!(v[0].rows[0].len(), 2);
    }
}

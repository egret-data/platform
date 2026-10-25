//! The source a new data app starts from (spec D13: the template pre-creates the
//! connection cell so the egret engine is listed and selected from the first run).

/// `database` must be a name the caller already resolved against the workspace's
/// real databases; `{:?}` escapes quotes and backslashes regardless.
pub fn new_app_source(database: Option<&str>) -> String {
    // Written the way marimo itself writes a notebook (a cell returns only what
    // another cell uses; `mo.md` with a triple-quoted block), so the save marimo
    // makes when the editor first opens leaves it byte-for-byte unchanged and
    // `is_untouched_starter` still recognises it (KI-042).
    let (returns, connect) = match database {
        Some(db) => (
            "egret, mo",
            format!(
                "\n\n@app.cell\ndef _(egret):\n    # Queries run with each viewer's own permissions.\n    db = egret.connect({db:?})\n    return\n"
            ),
        ),
        None => ("(mo,)", String::new()),
    };
    format!(
        r##"import marimo

__generated_with = "0.25.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import egret_marimo as egret

    return {returns}
{connect}

@app.cell
def _(mo):
    mo.md("""
    # New data app
    """)
    return


if __name__ == "__main__":
    app.run()
"##
    )
}

/// Whether `source` is exactly what [`new_app_source`] made, for no database or for
/// any one database name: a draft nobody has saved over yet.
pub fn is_untouched_starter(source: &str) -> bool {
    if source == new_app_source(None) {
        return true;
    }
    // Split the with-database template around its quoted name: `{:?}` of "\0" is
    // `"\0"`, which appears nowhere else in it.
    let marked = new_app_source(Some("\0"));
    let Some((head, tail)) = marked.split_once(r#""\0""#) else {
        return false;
    };
    source
        .strip_prefix(head)
        .and_then(|rest| rest.strip_suffix(tail))
        .is_some_and(|name| {
            name.len() >= 2 && name.starts_with('"') && name.ends_with('"') && !name.contains('\n')
        })
}

#[cfg(test)]
mod tests {
    use super::{is_untouched_starter, new_app_source};

    #[test]
    fn database_name_is_escaped_into_the_connect_call() {
        let src = new_app_source(Some("a\"b"));
        assert!(src.contains(r#"egret.connect("a\"b")"#), "{src}");
    }

    #[test]
    fn without_a_database_there_is_no_connect_cell() {
        assert!(!new_app_source(None).contains("egret.connect"));
    }

    #[test]
    fn the_starter_is_recognised_with_and_without_a_database() {
        assert!(is_untouched_starter(&new_app_source(None)));
        assert!(is_untouched_starter(&new_app_source(Some("sales"))));
        assert!(is_untouched_starter(&new_app_source(Some("a\"b"))));
    }

    /// What marimo 0.25.0 saved when the editor first opened a new app (read back
    /// from the draft on the demo stack). The starter must already be in that form,
    /// or the first open turns it into a "real" draft and it publishes after all.
    #[test]
    fn marimos_first_save_of_the_starter_is_still_the_starter() {
        let saved_without_db = "import marimo\n\n__generated_with = \"0.25.0\"\napp = marimo.App(width=\"medium\")\n\n\n@app.cell\ndef _():\n    import marimo as mo\n    import egret_marimo as egret\n\n    return (mo,)\n\n\n@app.cell\ndef _(mo):\n    mo.md(\"\"\"\n    # New data app\n    \"\"\")\n    return\n\n\nif __name__ == \"__main__\":\n    app.run()\n";
        assert_eq!(new_app_source(None), saved_without_db);
        let saved_with_db = "import marimo\n\n__generated_with = \"0.25.0\"\napp = marimo.App(width=\"medium\")\n\n\n@app.cell\ndef _():\n    import marimo as mo\n    import egret_marimo as egret\n\n    return egret, mo\n\n\n@app.cell\ndef _(egret):\n    # Queries run with each viewer's own permissions.\n    db = egret.connect(\"trino-demo\")\n    return\n\n\n@app.cell\ndef _(mo):\n    mo.md(\"\"\"\n    # New data app\n    \"\"\")\n    return\n\n\nif __name__ == \"__main__\":\n    app.run()\n";
        assert_eq!(new_app_source(Some("trino-demo")), saved_with_db);
        assert!(is_untouched_starter(saved_with_db));
    }

    #[test]
    fn any_edit_makes_it_a_real_draft() {
        let src = new_app_source(Some("sales"));
        assert!(!is_untouched_starter(
            &src.replace("# New data app", "# Revenue")
        ));
        assert!(!is_untouched_starter(&format!("{src}\n# note\n")));
        assert!(!is_untouched_starter(&src.replace(
            "egret.connect(\"sales\")",
            "egret.connect(\"sales\")\n    x = 1"
        )));
        assert!(!is_untouched_starter(""));
    }
}

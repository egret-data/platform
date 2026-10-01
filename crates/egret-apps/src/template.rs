//! The source a new data app starts from (spec D13: the template pre-creates the
//! connection cell so the egret engine is listed and selected from the first run).

/// `database` must be a name the caller already resolved against the workspace's
/// real databases; `{:?}` escapes quotes and backslashes regardless.
pub fn new_app_source(database: Option<&str>) -> String {
    let connect = match database {
        Some(db) => format!(
            "\n\n@app.cell\ndef _(egret):\n    # Queries run with each viewer's own permissions.\n    db = egret.connect({db:?})\n    return (db,)\n"
        ),
        None => String::new(),
    };
    format!(
        r##"import marimo

__generated_with = "0.25.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import egret_marimo as egret
    return egret, mo
{connect}

@app.cell
def _(mo):
    mo.md("# New data app")
    return


if __name__ == "__main__":
    app.run()
"##
    )
}

#[cfg(test)]
mod tests {
    use super::new_app_source;

    #[test]
    fn database_name_is_escaped_into_the_connect_call() {
        let src = new_app_source(Some("a\"b"));
        assert!(src.contains(r#"egret.connect("a\"b")"#), "{src}");
    }

    #[test]
    fn without_a_database_there_is_no_connect_cell() {
        assert!(!new_app_source(None).contains("egret.connect"));
    }
}

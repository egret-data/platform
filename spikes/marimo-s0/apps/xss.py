import marimo

__generated_with = "0.25.0"
app = marimo.App()


@app.cell
def _():
    import anywidget
    import marimo as mo
    import traitlets

    return anywidget, mo, traitlets


@app.cell
def _(anywidget, mo, traitlets):
    class Probe(anywidget.AnyWidget):
        _esm = """
        export default { async render({ el }) {
          const out = {};
          try { out.parentDoc = String(!!window.parent.document.title); }
          catch (e) { out.parentDoc = "blocked:" + e.name; }
          out.cookieVisible = document.cookie;
          try {
            const r = await fetch("http://egret.localhost:8700/api/whoami", { credentials: "include" });
            out.whoamiReadable = "status " + r.status + " " + (await r.text());
          } catch (e) { out.whoamiReadable = "blocked:" + e.name; }
          try {
            const r = await fetch("http://egret.localhost:8700/api/mutate",
              { method: "POST", credentials: "include", headers: { "x-egret-csrf": "evil" } });
            out.mutateWithHeader = "status " + r.status;
          } catch (e) { out.mutateWithHeader = "blocked:" + e.name; }
          document.cookie = "egret_csrf=evil; domain=egret.localhost; path=/";
          document.cookie = "__Host-egret_csrf=evil; domain=egret.localhost; path=/; secure";
          out.cookieAfterToss = document.cookie;
          try {
            await fetch("http://egret.localhost:8700/api/mutate",
              { method: "POST", credentials: "include", mode: "no-cors" });
            out.mutateNoCors = "sent";
          } catch (e) { out.mutateNoCors = "blocked:" + e.name; }
          // Extra (task 6): a run-mode apps-origin cookie must NOT reach the edit upstream.
          try {
            const r = await fetch("/e/api/status", { credentials: "include" });
            out.editApiStatus = "status " + r.status;
          } catch (e) { out.editApiStatus = "blocked:" + e.name; }
          try {
            const r = await fetch("/e/", { credentials: "include" });
            out.editRoot = "status " + r.status;
          } catch (e) { out.editRoot = "blocked:" + e.name; }
          el.innerHTML = "<pre id='xss-result'>XSSRESULT " + JSON.stringify(out) + "</pre>";
        } };
        """
        value = traitlets.Int(0).tag(sync=True)

    Probe()
    return


@app.cell
def _(mo):
    mo.Html("<div id='html-script'>HTMLSCRIPT pending</div><script>document.getElementById('html-script').textContent='HTMLSCRIPT executed'</script>")
    return


@app.cell
def _(mo):
    mo.Html("<div id='html-onerror'>ONERROR pending</div><img src=x onerror=\"document.getElementById('html-onerror').textContent='ONERROR executed'\">")
    return


if __name__ == "__main__":
    app.run()

FROM python:3.12-slim
ARG MARIMO_VERSION=0.25.0
# The package is built from a separate dir: a pyproject.toml in /work, a parent of /work
# or $HOME makes the launcher refuse to start (it would shadow the locked marimo config).
WORKDIR /opt/egret-apps/src
COPY requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt
COPY pyproject.toml ./
COPY egret_marimo ./egret_marimo
RUN pip install --no-cache-dir --no-deps . \
 && cd / && rm -rf /opt/egret-apps/src
WORKDIR /opt/egret-apps
COPY venv-shim ./venv-shim
COPY config ./config
RUN chmod +x venv-shim/bin/python \
 && useradd --uid 1000 --create-home --home-dir /home/egret egret \
 && mkdir -p /work && chown -R 1000:1000 /home/egret /work \
 && python -c "import marimo,sys; sys.exit(marimo.__version__ != '${MARIMO_VERSION}')" \
 && cd /tmp && python -I -c "import egret_marimo,sys; sys.exit('/work' in egret_marimo.__file__)"
ENV HOME=/home/egret \
    XDG_CONFIG_HOME=/opt/egret-apps/config \
    MARIMO_SKIP_UPDATE_CHECK=1 \
    PYTHONNOUSERSITE=1 \
    EGRET_REAL_PYTHON=/usr/local/bin/python \
    EGRET_MARIMO_VERSION=${MARIMO_VERSION} \
    EGRET_VENV_SHIM=/opt/egret-apps/venv-shim \
    APPS_WORKDIR=/work
WORKDIR /work
USER 1000
EXPOSE 2718 2719
# -I: phase 1 holds the secrets in its environment, so it must not import from the cwd either.
ENTRYPOINT ["python", "-I", "-m", "egret_marimo.launcher"]

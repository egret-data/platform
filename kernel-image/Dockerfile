# Egret notebook kernel image: Jupyter Kernel Gateway + Daft + pyiceberg + helpers.
# D1b spawns `jupyter kernelgateway`; D1c proxies its channels WS. Helpers are
# auto-loaded into every Python kernel via an IPython startup file.
FROM python:3.11-slim

WORKDIR /opt/egret-kernel

# Runtime deps. Daft + pyiceberg are the engine/catalog libraries; the gateway
# is the kernel front door D1b/D1c talk to; ipykernel provides the python3
# kernelspec the gateway launches (and pulls IPython, which display() uses).
# sqlglot is required by daft.read_sql; sqlalchemy + pymysql + trino back the
# trino/starrocks connection factories (trino provides the SQLAlchemy dialect +
# JWTAuthentication); pandas backs write_table (Daft's write_sql → pandas.to_sql).
RUN pip install --no-cache-dir jupyter_kernel_gateway ipykernel daft "pyiceberg[s3fs]" pymysql sqlglot sqlalchemy trino pandas

# Install the helper package.
COPY pyproject.toml ./
COPY egret_kernel_helpers ./egret_kernel_helpers
RUN pip install --no-cache-dir .

# Auto-load helpers into every kernel (IPython startup runs these at kernel boot).
# Run as a non-root user (uid 1000) so the pod satisfies the hardened
# securityContext (runAsNonRoot / runAsUser=1000) + the `restricted` PodSecurity
# namespace. jupyter writes its runtime under $HOME, so uid 1000 needs a real,
# owned home dir — a root-only image (HOME=/root, files in /root/.ipython) fails
# with "cannot create /.local/share/jupyter/runtime" when run as 1000.
RUN useradd --uid 1000 --create-home --home-dir /home/egret egret \
 && mkdir -p /home/egret/.ipython/profile_default/startup
COPY startup/00-egret.py /home/egret/.ipython/profile_default/startup/00-egret.py

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh \
 && chown -R 1000:1000 /home/egret /opt/egret-kernel

ENV HOME=/home/egret
USER 1000

EXPOSE 8888
ENTRYPOINT ["/entrypoint.sh"]

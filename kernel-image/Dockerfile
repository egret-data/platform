# Egret notebook kernel image: Jupyter Kernel Gateway + Daft + pyiceberg + helpers.
# D1b spawns `jupyter kernelgateway`; D1c proxies its channels WS. Helpers are
# auto-loaded into every Python kernel via an IPython startup file.
FROM python:3.11-slim

WORKDIR /opt/egret-kernel

# Runtime deps. Daft + pyiceberg are the engine/catalog libraries; the gateway
# is the kernel front door D1b/D1c talk to; ipykernel provides the python3
# kernelspec the gateway launches (and pulls IPython, which display() uses).
# sqlglot is required by daft.read_sql; sqlalchemy + pymysql back the trino/starrocks connection factories.
RUN pip install --no-cache-dir jupyter_kernel_gateway ipykernel daft "pyiceberg[s3fs]" pymysql sqlglot sqlalchemy

# Install the helper package.
COPY pyproject.toml ./
COPY egret_kernel_helpers ./egret_kernel_helpers
RUN pip install --no-cache-dir .

# Auto-load helpers into every kernel (IPython startup runs these at kernel boot).
RUN mkdir -p /root/.ipython/profile_default/startup
COPY startup/00-egret.py /root/.ipython/profile_default/startup/00-egret.py

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 8888
ENTRYPOINT ["/entrypoint.sh"]

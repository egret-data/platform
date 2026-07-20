"""Engine TLS pinning helpers — pure (no daft/pymysql/sqlalchemy needed)."""
import os

from egret_kernel_helpers.engines import _tls_verify, _starrocks_ssl_kwargs, _write_temp_pem

_PEM = "-----BEGIN CERTIFICATE-----\nFAKE\n-----END CERTIFICATE-----\n"


def test_write_temp_pem_roundtrips():
    path = _write_temp_pem(_PEM)
    assert path.endswith(".pem")
    with open(path) as f:
        assert f.read() == _PEM
    os.unlink(path)


def test_tls_verify_pins_cert_by_default():
    # a delivered cert → a CA file path (Trino/requests verifies against it)
    v = _tls_verify(_PEM, insecure=False)
    assert isinstance(v, str) and v.endswith(".pem")
    os.unlink(v)


def test_tls_verify_true_when_no_cert():
    assert _tls_verify(None, insecure=False) is True  # system CA bundle


def test_tls_verify_false_only_on_explicit_optout():
    assert _tls_verify(_PEM, insecure=True) is False  # break-glass, off by default


def test_starrocks_ssl_pins_cert_and_skips_hostname():
    k = _starrocks_ssl_kwargs(_PEM, insecure=False)
    assert k["ssl"]["ca"].endswith(".pem")          # pin the CA
    assert k["ssl_verify_identity"] is False          # skip hostname/SAN (self-signed FE)
    os.unlink(k["ssl"]["ca"])


def test_starrocks_ssl_insecure_optout_disables_verify():
    k = _starrocks_ssl_kwargs(_PEM, insecure=True)
    assert k["ssl_verify_cert"] is False


def test_starrocks_ssl_no_cert_is_plain():
    assert _starrocks_ssl_kwargs(None, insecure=False) == {}

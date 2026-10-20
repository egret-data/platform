#!/bin/sh
# DEMO ONLY. Bring the schema up, create the first admin and workspace if missing, then serve.
# `egret init` is idempotent: without --force it does nothing when a platform_admin exists.
set -eu
egret migrate
egret init
exec egret serve

-- ibis_testing (ibis-rs fixtures) is created by POSTGRES_DB.
-- LEVENSHTEIN (Ibis `levenshtein`) is in an extension that ships with Postgres.
CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;
-- ibis_python: where conformance/gen_goldens.py loads fixtures for Python Ibis.
CREATE DATABASE ibis_python;
\connect ibis_python
CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;

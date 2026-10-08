#!/usr/bin/env python3
"""hlconf.py : lecture de .env et de homelab.toml pour les outils et scripts Python (2026-10-08, lot 4).

Mêmes règles que homelabd et homelabctl (`homelab_core::config::Config::load`) : une valeur de homelab.toml peut citer
une variable de .env, `${NOM}`, remplacée au chargement (environnement du processus d'abord, puis .env) ; `$${` écrit
un « ${ » littéral ; tout autre `$` reste tel quel ; une valeur qui finit par `/` suivie d'un `/` du modèle n'en
garde qu'un. Variable absente ou vide = ConfError qui nomme la variable et les clés qui la citent, jamais une valeur.
Ce qui désigne la seedbox (adresse publique, compte, dossier personnel) vit ainsi hors du dépôt public.

    sys.path.insert(0, '<dépôt>/tools/lib'); import hlconf
    env = hlconf.read_env('/opt/homelab/.env')
    cfg = hlconf.load_toml('/opt/homelab/homelab.toml', env)
"""
import os
import re
import tomllib

_NAME = re.compile(r'^[A-Za-z_][A-Za-z0-9_]*$')


class ConfError(Exception):
    """Configuration inutilisable ; le message ne contient jamais de valeur."""


class _Missing(Exception):
    def __init__(self, name):
        super().__init__(name)
        self.name = name


def read_env(path):
    """.env → dict (lignes `CLÉ=valeur`, commentaires `#` en début de ligne, guillemets retirés). Mêmes règles que les
    lecteurs historiques des scripts : pas de commentaire en fin de ligne."""
    out = {}
    with open(path, encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith('#') and '=' in line:
                k, v = line.split('=', 1)
                out[k.strip()] = v.strip().strip('"').strip("'")
    return out


def lookup_from(env=None, env_file=None):
    """Variable de l'environnement du processus, sinon de `env` ou du fichier `env_file` (lu seulement au premier
    besoin ; illisible = vide), comme dotenvy : .env n'écrase jamais une variable déjà définie. Vide = absente."""
    cache = {'env': env}

    def values():
        if cache['env'] is None:
            try:
                cache['env'] = read_env(env_file) if env_file else {}
            except OSError:
                cache['env'] = {}
        return cache['env']

    def look(name):
        v = os.environ[name] if name in os.environ else values().get(name)
        v = (v or '').strip()
        return v or None
    return look


def expand(s, look):
    """`${NOM}` → valeur ; lève _Missing(nom) ou ValueError (syntaxe)."""
    out = []
    rest = s
    while True:
        i = rest.find('$')
        if i < 0:
            out.append(rest)
            return ''.join(out)
        out.append(rest[:i])
        after = rest[i + 1:]
        if after.startswith('${'):
            out.append('${')
            rest = after[2:]
        elif after.startswith('{'):
            end = after.find('}')
            if end < 0 or not _NAME.match(after[1:end]):
                raise ValueError('« ${ » mal formé (attendu ${NOM}, ou $${ pour un « ${ » littéral)')
            name = after[1:end]
            value = look(name)
            if value is None:
                raise _Missing(name)
            rest = after[end + 1:]
            if rest.startswith('/') and value.endswith('/'):
                value = value[:-1]
            out.append(value)
        else:
            out.append('$')
            rest = after


def expand_tree(obj, look):
    """Remplace `${NOM}` dans toutes les chaînes ; relève toutes les variables manquantes avant l'erreur."""
    missing = {}

    def walk(o, path):
        if isinstance(o, dict):
            return {k: walk(v, '%s.%s' % (path, k) if path else k) for k, v in o.items()}
        if isinstance(o, list):
            return [walk(v, '%s[%d]' % (path, i)) for i, v in enumerate(o)]
        if isinstance(o, str) and '$' in o:
            try:
                return expand(o, look)
            except _Missing as e:
                missing.setdefault(e.name, []).append(path)
                return o
            except ValueError as e:
                raise ConfError('%s : %s' % (path, e)) from None
        return o

    out = walk(obj, '')
    if missing:
        parts = []
        for name in sorted(missing):
            keys = missing[name]
            parts.append('%s (%s%s)' % (name, ', '.join(keys[:3]), ', …' if len(keys) > 3 else ''))
        raise ConfError("variable(s) d'environnement absente(s) ou vide(s), à définir dans .env : "
                        + ' ; '.join(parts))
    return out


def load_toml(path, env=None, env_file=None):
    """homelab.toml lu, `${NOM}` remplacés (environnement, puis `env` — en général `read_env(.env)` — ou le fichier
    `env_file`, lu seulement si une variable manque à l'environnement)."""
    with open(path, 'rb') as f:
        raw = tomllib.load(f)
    return expand_tree(raw, lookup_from(env, env_file))

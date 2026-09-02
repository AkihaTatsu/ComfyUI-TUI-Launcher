"""In-memory HuggingFace URL rewrite wrapper for ComfyUI.

This file is embedded into the Rust binary and executed via `python -c`.
It must not rely on local file paths.
"""

import functools
import os
import runpy
import sys
from urllib.parse import urlsplit


ENV_NAME = "COMFYUI_TUI_HF_MIRROR"
HF_HOSTS = {"huggingface.co", "www.huggingface.co"}


def _valid_mirror(value):
    try:
        parsed = urlsplit(value)
    except Exception:
        return False
    return parsed.scheme in {"http", "https"} and bool(parsed.netloc)


MIRROR = os.environ.get(ENV_NAME, "").rstrip("/")


def _rewrite_url(url):
    if not MIRROR or not isinstance(url, str):
        return url
    try:
        parsed = urlsplit(url)
    except Exception:
        return url
    if parsed.scheme not in {"http", "https"}:
        return url
    if parsed.netloc.lower() not in HF_HOSTS:
        return url

    path = parsed.path or ""
    if not path.startswith("/"):
        path = "/" + path
    rewritten = MIRROR + path
    if parsed.query:
        rewritten += "?" + parsed.query
    if parsed.fragment:
        rewritten += "#" + parsed.fragment
    return rewritten


def _patch_urllib():
    try:
        import urllib.request

        original_request_init = urllib.request.Request.__init__

        @functools.wraps(original_request_init)
        def request_init(self, url, *args, **kwargs):
            return original_request_init(self, _rewrite_url(url), *args, **kwargs)

        urllib.request.Request.__init__ = request_init

        original_urlopen = urllib.request.urlopen

        @functools.wraps(original_urlopen)
        def urlopen(url, *args, **kwargs):
            return original_urlopen(_rewrite_url(url), *args, **kwargs)

        urllib.request.urlopen = urlopen
    except Exception:
        pass


def _patch_requests():
    try:
        import requests.sessions

        original_request = requests.sessions.Session.request

        @functools.wraps(original_request)
        def request(self, method, url, *args, **kwargs):
            return original_request(self, method, _rewrite_url(url), *args, **kwargs)

        requests.sessions.Session.request = request
    except Exception:
        pass


def _patch_httpx():
    try:
        import httpx

        original_request = httpx.Client.request
        original_async_request = httpx.AsyncClient.request

        @functools.wraps(original_request)
        def request(self, method, url, *args, **kwargs):
            return original_request(self, method, _rewrite_url(str(url)), *args, **kwargs)

        @functools.wraps(original_async_request)
        async def async_request(self, method, url, *args, **kwargs):
            return await original_async_request(
                self, method, _rewrite_url(str(url)), *args, **kwargs
            )

        httpx.Client.request = request
        httpx.AsyncClient.request = async_request
    except Exception:
        pass


def _patch_aiohttp():
    try:
        import aiohttp

        original_request = aiohttp.ClientSession._request

        @functools.wraps(original_request)
        async def request(self, method, str_or_url, *args, **kwargs):
            return await original_request(
                self, method, _rewrite_url(str(str_or_url)), *args, **kwargs
            )

        aiohttp.ClientSession._request = request
    except Exception:
        pass


def _install():
    if not _valid_mirror(MIRROR):
        return
    _patch_urllib()
    _patch_requests()
    _patch_httpx()
    _patch_aiohttp()


def _run_main():
    if len(sys.argv) < 2:
        raise SystemExit("missing ComfyUI main.py path")
    main_py = sys.argv[1]
    sys.argv = sys.argv[1:]
    runpy.run_path(main_py, run_name="__main__")


_install()
_run_main()

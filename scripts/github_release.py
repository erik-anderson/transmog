"""Narrow GitHub release API requests, with errors never exposing credentials."""
import json
import os
import re
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


class ReleaseRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        original, destination = urlsplit(req.full_url), urlsplit(newurl)
        if original.scheme == "https" and destination.scheme != "https":
            raise RuntimeError("Refusing a non-HTTPS GitHub release redirect.")
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is not None and (original.scheme, original.netloc) != (destination.scheme, destination.netloc):
            redirected.remove_header("Authorization")
        return redirected


urlopen = build_opener(ReleaseRedirect()).open


def api_root(repository):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Expected a GitHub owner/repository name.")
    return "https://api.github.com/repos/" + repository


def request(url, *, method="GET", data=None, binary=False):
    headers = {"Accept": "application/octet-stream" if binary else "application/vnd.github+json",
               "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "Transmog-release-tools"}
    if os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    payload = None if data is None else json.dumps(data).encode("utf-8")
    if payload is not None:
        headers["Content-Type"] = "application/json"
    try:
        with urlopen(Request(url, data=payload, headers=headers, method=method), timeout=60) as response:
            result = response.read()
    except HTTPError as error:
        if method == "GET" and error.code == 404:
            return None
        raise RuntimeError(f"GitHub {method} failed with HTTP {error.code}.") from error
    return result if binary else json.loads(result)

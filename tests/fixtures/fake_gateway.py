#!/usr/bin/env python3
"""Fake Floway gateway for smoke-testing floway-cli.

Serves GET /v1/models (the public model superset the harness converters
consume) with the same fixture payload Floway's own agent-setup tests use.
The bearer token must match FLOWAY_TOKEN to exercise auth.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

TOKEN = "fw-test-key-1234"

MODELS = {
    "object": "list",
    "has_more": False,
    "data": [
        {
            "id": "gpt-5.6",
            "object": "model",
            "type": "model",
            "display_name": "GPT-5.6",
            "limits": {"max_context_window_tokens": 400000, "max_prompt_tokens": 300000, "max_output_tokens": 100000},
            "kind": "chat",
            "endpoints": {},
            "chat": {
                "modalities": {"input": ["text", "image"], "output": ["text"]},
                "reasoning": {"effort": {"supported": ["low", "medium", "high"], "default": "medium"}},
            },
            "pricing": {"entries": [{"rates": {"input_tokens": "0.0000025", "output_tokens": "0.000015", "input_cache_read_tokens": "0.00000025", "input_cache_write_tokens": "0.000003125"}}]},
        },
        {
            "id": "claude-opus-4-6",
            "object": "model",
            "type": "model",
            "display_name": "Claude Opus 4.6",
            "limits": {"max_context_window_tokens": 1000000, "max_output_tokens": 64000},
            "kind": "chat",
            "endpoints": {},
            "chat": {"modalities": {"input": ["text"], "output": ["text"]}},
        },
        {
            "id": "deepseek-v4-flash",
            "object": "model",
            "type": "model",
            "display_name": "DeepSeek V4 Flash",
            "limits": {"max_context_window_tokens": 1000000, "max_output_tokens": 64000},
            "kind": "chat",
            "endpoints": {},
            "chat": {"reasoning": {"effort": {"supported": ["high", "max"], "default": "high"}}},
        },
        {
            "id": "text-embedding-3",
            "object": "model",
            "type": "model",
            "kind": "embedding",
            "endpoints": {},
        },
    ],
}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/v1/models":
            self.send_response(404)
            self.end_headers()
            return
        auth = self.headers.get("Authorization", "")
        if auth != f"Bearer {TOKEN}":
            self.send_response(401)
            self.end_headers()
            return
        body = json.dumps(MODELS).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18099
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()

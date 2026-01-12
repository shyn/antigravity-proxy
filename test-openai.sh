#!/bin/bash

curl -X POST http://localhost:8045/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.5-flash",
    "messages": [
      {"role": "user", "content": "Hello, say hi back in one word"}
    ],
    "max_tokens": 50
  }'

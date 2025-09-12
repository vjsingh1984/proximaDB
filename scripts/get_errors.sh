#!/bin/bash
cargo check --tests 2>&1 | grep -A5 "error\[E"
#!/bin/bash
# Wrapper script to run axiomvault-desktop with correct library path
# This avoids conflicts with Anaconda's older GLib version

LD_LIBRARY_PATH=/lib64:/usr/lib64 ./target/debug/axiomvault-desktop "$@"

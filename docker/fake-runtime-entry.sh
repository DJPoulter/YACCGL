#!/bin/sh
# Stand-in for the Steam Linux Runtime entry point: skip our options and run the tool.
echo "runtime-args=$*" > /tmp/runtime-called.txt
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
exec "$@"

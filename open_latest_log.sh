#!/bin/bash

# Find the most recently modified file in ~/.coder
latest_file=$(ls -t ~/.coder | head -1)

if [ -z "$latest_file" ]; then
    echo "No files found in ~/.coder"
    exit 1
fi

# Open the latest file with less
less ~/.coder/"$latest_file"
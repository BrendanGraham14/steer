#!/bin/bash

# Find the most recently modified file in ~/.steer matching the datetime log file pattern: YYYYMMDD_HHMMSS.log
latest_file=$(ls -t ~/.steer/*.log 2>/dev/null | grep -E '/[0-9]{8}_[0-9]{6}\.log$' | head -1)

if [ -z "$latest_file" ]; then
    echo "No log files matching pattern YYYYMMDD_HHMMSS.log found in ~/.steer"
    exit 1
fi

# Open the latest matching log file with less
less "$latest_file"

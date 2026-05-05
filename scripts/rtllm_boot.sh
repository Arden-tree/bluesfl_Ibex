if [ -z "$1" ]; then
  echo "Error: No path provided."
  exit 1
fi

if [ ! -d "$1" ]; then
  echo "Error: Directory '$1' does not exist."
  exit 1
fi

if [ ! -f "$1/makefile" ]; then
  echo "Error: File 'makefile' not found in '$1'."
  exit 1
fi

cd "$1" || exit 1

# Rename verified_xx.v -> xx.v, and mutate on xx.v
for file in *; do
    if [[ $file =~ ^verified_(.*)$ ]]; then
        # Use match array for zsh compatibility
        if [[ -n "$BASH_VERSION" ]]; then
            new_name="${BASH_REMATCH[1]}"
        else
            new_name="${match[1]}"
        fi
        echo "Renaming: $file -> $new_name"
        mv "$file" "$new_name"
    fi
done

timeout 2s make iverilog
timeout 2s make sim

if cat run.log | grep -i -E "failed|error|failure"; then
  cat run.log > mismatch_log.txt
  echo "Found mismatch"
  exit 0
else
  echo "Not found mismatch"
  exit 1
fi

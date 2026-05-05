#!/usr/bin/env python3
import json
import argparse
from collections import Counter


def parse_json_file(file_path):
    """Parse the input JSON file and return the loaded data."""
    try:
        with open(file_path, 'r') as f:
            data = json.load(f)
        return data
    except FileNotFoundError:
        print(f"Error: File '{file_path}' not found.")
        exit(1)
    except json.JSONDecodeError:
        print(f"Error: File '{file_path}' is not a valid JSON file.")
        exit(1)


def extract_scopes(data):
    """Extract all scopes from the JSON data."""
    scopes = []
    for entry in data:
        if "scope" in entry:
            scopes.append(entry["scope"])
    return scopes


def main():
    parser = argparse.ArgumentParser(description="Parse JSON file and display scopes.")
    parser.add_argument("-i", "--input-file", help="Path to the input JSON file")
    parser.add_argument("-s", "--show-scopes", action="store_true",
                        help="Show all scopes included in the file")

    args = parser.parse_args()

    # Parse the JSON file
    data = parse_json_file(args.input_file)

    # Extract scopes
    scopes = extract_scopes(data)

    # Count unique scopes
    scope_counter = Counter(scopes)

    # Display results
    if args.show_scopes:
        print(f"Found {len(scope_counter)} unique scope(s):")
        for i, (scope, count) in enumerate(scope_counter.items(), 1):
            print(f"{i}. {scope} (appears {count} time(s))")
    else:
        print(f"Found {len(scope_counter)} unique scope(s). Use -s to display them.")


if __name__ == "__main__":
    main()
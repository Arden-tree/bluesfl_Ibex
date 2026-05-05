import json
import argparse

def collect_bid(data):
    res = []
    for d in data:
        res.append(d['bid'])
    return res

def main():
    # Set up argument parser
    parser = argparse.ArgumentParser(description='Compare bid data between two JSON files.')
    parser.add_argument('file1', help='First JSON file path')
    parser.add_argument('file2', help='Second JSON file path')

    # Parse arguments
    args = parser.parse_args()

    # Load data from both files
    with open(args.file1, 'r') as fp:
        data1 = json.load(fp)

    with open(args.file2, 'r') as fp:
        data2 = json.load(fp)

    # Collect bids from both datasets
    d1_bids = collect_bid(data1)
    d2_bids = collect_bid(data2)

    print(f"d1 bids len: {len(d1_bids)}")
    print(f"d2 bids len: {len(d2_bids)}")

    # Check for bids in file1 but not in file2
    print(f"\nBids in {args.file1} but not in {args.file2}:")
    found_mismatch1 = False
    for bid in d1_bids:
        if bid not in d2_bids:
            found_mismatch1 = True
            print(f"bid in {args.file1}, not in {args.file2}: {bid}")

            for d in data1:
                if d["bid"] == bid:
                    print(d)

    if not found_mismatch1:
        print("None")

    # Check for bids in file2 but not in file1
    print(f"\nBids in {args.file2} but not in {args.file1}:")
    found_mismatch2 = False
    for bid in d2_bids:
        if bid not in d1_bids:
            found_mismatch2 = True
            print(f"bid in {args.file2}, not in {args.file1}: {bid}")
            for d in data2:
                if d["bid"] == bid:
                    print(d)

    if not found_mismatch2:
        print("None")

if __name__ == "__main__":
    main()
import json
import graphviz
import argparse
import sys
from typing import Dict, List, Any, Set, Tuple

def extract_identifier_value(node: str) -> str:
    node = eval(node)
    """Extract the identifier name from a node."""
    if "Identifier" in node:
        ident = node["Identifier"]
        return f"{ident[1]}@{ident[0]['line']}"
    return str(node)  # Fallback in case structure is different

def create_graph_from_blocks(blocks: List[Dict]) -> graphviz.Digraph:
    """Create a directed graph based on block connections, grouped by bid."""
    dot = graphviz.Digraph(comment='Block Connections')
    dot.attr('graph', rankdir='LR')
    dot.attr('node', shape='box')

    block_inputs: Dict[str, Dict] = {}
    block_suspicious: Dict[str, Set[str]] = {}
    blocks_by_bid: Dict[int, List[Dict]] = {}

    # Group blocks by bid
    for block in blocks:
        bid = block.get("bid")
        blocks_by_bid.setdefault(bid, []).append(block)

    # Create clusters for each bid group
    for bid, block_group in blocks_by_bid.items():
        with dot.subgraph(name=f'cluster_{bid}') as sub:
            sub.attr(label=f'Block {bid}', style='dashed')

            for block in block_group:
                block_time = block.get("time")
                node_id = f"{bid}_{block_time}"
                scope_name = block.get("scope").split('.')[-1]
                label = f"Block {bid}\nType: {block.get('type')}\nTime: {block_time}\nScope: {scope_name}"

                sub.node(node_id, label)

                # input_vars_time = block.get("suspicious_input_time", 0)
                input_vars = {(str(v), t) for v, t in block.get("suspicious_input_vars_with_time", [])}
                suspicious_identifiers = {str(v) for v in block.get("suspicious_outputs", [])}

                block_inputs[node_id] = {"vars": input_vars}
                block_suspicious[node_id] = suspicious_identifiers

    # Create edges
    for block_a in blocks:
        bid_a = block_a.get("bid")
        time_a = block_a.get("time")
        node_id_a = f"{bid_a}_{time_a}"
        inputs_a = block_inputs.get(node_id_a, {})
        inputs_a_vars = inputs_a.get("vars", set())

        for vars, t in inputs_a_vars:
            for block_b in blocks:
                bid_b = block_b.get("bid")
                time_b = block_b.get("time")
                node_id_b = f"{bid_b}_{time_b}"
                if node_id_a == node_id_b:
                    continue
                if t != time_b:
                    continue

                suspicious_b = block_suspicious.get(node_id_b, set())
                common_identifiers = {vars}.intersection(suspicious_b)

                if common_identifiers:
                    dot.edge(
                        node_id_b,
                        node_id_a,
                        label=", ".join(map(extract_identifier_value, common_identifiers))
                    )

    return dot

def create_scope_graph_from_blocks(blocks: List[Dict]) -> graphviz.Digraph:
    """Create a directed graph where nodes are scopes and edges represent connections between scopes."""
    dot = graphviz.Digraph(comment='Scope Connections')
    dot.attr('graph', rankdir='LR')
    dot.attr('node', shape='box')

    # Maps for storing block information
    block_inputs: Dict[str, Dict] = {}
    block_suspicious: Dict[str, Set[str]] = {}
    scope_connections: Dict[Tuple[str, str], Set[str]] = {}  # (from_scope, to_scope) -> set of labels
    scopes: Set[str] = set()

    # Process block information
    for block in blocks:
        bid = block.get("bid")
        block_time = block.get("time")
        node_id = f"{bid}_{block_time}"
        scope = block.get("scope")
        scopes.add(scope)

        # input_vars_time = block.get("suspicious_input_time", 0)
        input_vars = {(str(v), t) for v, t in block.get("suspicious_input_vars_with_time", [])}
        suspicious_identifiers = {str(v) for v in block.get("suspicious_outputs", [])}

        block_inputs[node_id] = {"vars": input_vars, "scope": scope}
        block_suspicious[node_id] = suspicious_identifiers

    # Add nodes for each scope
    for scope in scopes:
        # Use last part of scope path for label for better readability
        label = scope.split('.')[-1]
        dot.node(scope, f"{label}\n({scope})")

    # Find connections between scopes
    for block_a in blocks:
        bid_a = block_a.get("bid")
        time_a = block_a.get("time")
        node_id_a = f"{bid_a}_{time_a}"
        scope_a = block_a.get("scope")

        inputs_a = block_inputs.get(node_id_a, {})
        inputs_a_vars = inputs_a.get("vars", set())

        for vars, t in inputs_a_vars:
            for block_b in blocks:
                bid_b = block_b.get("bid")
                time_b = block_b.get("time")
                node_id_b = f"{bid_b}_{time_b}"
                scope_b = block_b.get("scope")

                if node_id_a == node_id_b:
                    continue
                if t != time_b:
                    continue

                suspicious_b = block_suspicious.get(node_id_b, set())
                common_identifiers = {vars}.intersection(suspicious_b)

                if common_identifiers:
                    # For each common identifier, extract its value
                    for common_id in common_identifiers:
                        connection_key = (scope_b, scope_a)
                        # Initialize the set if this connection doesn't exist yet
                        if connection_key not in scope_connections:
                            scope_connections[connection_key] = set()
                        # Add the identifier to the set of identifiers for this connection
                        scope_connections[connection_key].add(extract_identifier_value(common_id))

    # Create edges from scope connections, combining all identifiers for the same scope pair
    for (from_scope, to_scope), identifiers in scope_connections.items():
        # Join all unique identifiers with commas for the edge label
        combined_label = ""
        dot.edge(from_scope, to_scope, label=combined_label)

    return dot

def main():
    # Set up argument parser
    parser = argparse.ArgumentParser(description='Generate a block connection graph from JSON data')
    parser.add_argument('-i', '--input_file', help='Path to the JSON file containing block data')
    parser.add_argument('-o', '--output', default='block_graph',
                        help='Output filename (without extension, default: block_graph)')
    parser.add_argument('-f', '--format', default='png', choices=['png', 'svg', 'pdf'],
                        help='Output format (default: png)')
    parser.add_argument('-s', '--scope', action='store_true',
                        help='Generate scope-only graph instead of block graph')
    parser.add_argument('--show-dot', action='store_true',
                        help='Print the DOT representation to stdout')

    args = parser.parse_args()

    # Load JSON data (required)
    try:
        with open(args.input_file, 'r') as f:
            blocks = json.load(f)
        print(f"Loaded data from {args.input_file}")
    except FileNotFoundError:
        print(f"Error: File {args.input_file} not found.")
        sys.exit(1)
    except json.JSONDecodeError:
        print(f"Error: Invalid JSON format in file {args.input_file}")
        sys.exit(1)

    # Create and render the graph based on the mode
    if args.scope:
        dot = create_scope_graph_from_blocks(blocks)
        output_file = f"{args.output}_scope"
        print("Generating scope-only graph...")
    else:
        dot = create_graph_from_blocks(blocks)
        output_file = args.output
        print("Generating block graph...")

    output_format = args.format

    try:
        dot.render(output_file, format=output_format, cleanup=True, view=True)
        print(f"Graph created and saved as '{output_file}.{output_format}'")
    except Exception as e:
        print(f"Error rendering graph: {e}")
        sys.exit(1)

    # Print DOT representation if requested
    if args.show_dot:
        print("\nGraph DOT representation:")
        print(dot.source)

if __name__ == "__main__":
    main()
#!/usr/bin/env python3
"""
Generate signal_name_map.json from NutShell Chisel-generated Verilog files.

For each module, extract port names and create canonical short names:
  - io_in_bits_decode_cf_redirect_target -> redirect_target
  - io_out_bits_result -> result
  - io_redirect_valid -> valid (too short, skip)

Also extracts the mapping between parent-side and child-side signal names
from module instantiations.
"""

import os
import re
import json
import sys
from pathlib import Path


def extract_canonical_name(port_name: str) -> str | None:
    """Extract a canonical short name from a Chisel-generated port name."""
    # Remove common prefixes
    name = port_name
    for prefix in ["io_in_bits_", "io_out_bits_", "io_in_", "io_out_", "io_"]:
        if name.startswith(prefix):
            name = name[len(prefix):]
            break

    # Skip if too short (< 4 chars after stripping)
    if len(name) < 4:
        return None

    # Skip generic names that would match too many things
    generic = {"data", "valid", "ready", "bits", "clock", "reset"}
    if name in generic:
        return None

    return name


def parse_module_ports(filepath: str) -> dict:
    """Parse a Verilog file and extract module name and port names."""
    with open(filepath, 'r') as f:
        content = f.read()

    # Find module declaration
    mod_match = re.search(r'module\s+(\w+)\s*\(', content)
    if not mod_match:
        return None

    module_name = mod_match.group(1)

    # Extract the port list section (between module( and the closing );)
    port_section_start = mod_match.end()
    # Find the matching ); for the module port list
    paren_depth = 1
    pos = port_section_start
    while pos < len(content) and paren_depth > 0:
        if content[pos] == '(':
            paren_depth += 1
        elif content[pos] == ')':
            paren_depth -= 1
        pos += 1
    port_section = content[port_section_start:pos]

    # Extract all identifiers that look like signal names (io_*, gateway*, *_bore)
    # CIRCT generates ports like:
    #   input [63:0] io_in_bits_src1,
    #                io_in_bits_src2,   <-- no direction prefix
    #   output io_redirect_target,
    ports = {}
    id_pattern = re.compile(r'\b(io_\w+|gateway\w+|\w+_bore)\b')

    for match in id_pattern.finditer(port_section):
        port_name = match.group(1)
        if port_name in ('clock', 'reset', 'io_clock', 'io_reset'):
            continue
        canonical = extract_canonical_name(port_name)
        if canonical:
            ports[port_name] = canonical

    return {
        "module": module_name,
        "ports": ports
    }


def parse_instantiation_mappings(filepath: str) -> list:
    """Parse module instantiations to get parent-child signal mappings."""
    with open(filepath, 'r') as f:
        content = f.read()

    mappings = []
    # Match: ModuleName instance_name (
    inst_pattern = re.compile(
        r'(\w+)\s+(\w+)\s*\((.*?)\)\s*;',
        re.DOTALL
    )

    for match in inst_pattern.finditer(content):
        module_name = match.group(1)
        instance_name = match.group(2)
        connections_str = match.group(3)

        # Parse port connections: .port_name(signal_name)
        conn_pattern = re.compile(r'\.(\w+)\s*\(\s*([^)]+)\s*\)')
        connections = {}
        for conn in conn_pattern.finditer(connections_str):
            port_name = conn.group(1)
            signal_name = conn.group(2).strip()
            if signal_name and not signal_name.startswith('{'):
                connections[port_name] = signal_name

        if connections:
            mappings.append({
                "module": module_name,
                "instance": instance_name,
                "connections": connections
            })

    return mappings


def main():
    rtl_dir = sys.argv[1] if len(sys.argv) > 1 else "/home/yuan/NutShell/build/rtl"
    output_path = sys.argv[2] if len(sys.argv) > 2 else "/home/yuan/bluesfl/signal_name_map.json"

    result = {
        "modules": {},  # module_name -> {port_name: canonical_name}
        "instantiations": {}  # parent_module -> [{module, instance, connections}]
    }

    sv_files = sorted(Path(rtl_dir).glob("*.sv"))

    for sv_file in sv_files:
        # Parse ports
        port_info = parse_module_ports(str(sv_file))
        if port_info and port_info["ports"]:
            result["modules"][port_info["module"]] = port_info["ports"]

        # Parse instantiations
        inst_mappings = parse_instantiation_mappings(str(sv_file))
        if inst_mappings:
            module_name = port_info["module"] if port_info else sv_file.stem
            result["instantiations"][module_name] = inst_mappings

    with open(output_path, 'w') as f:
        json.dump(result, f, indent=2)

    print(f"Generated {output_path}")
    print(f"  Modules with ports: {len(result['modules'])}")
    print(f"  Modules with instantiations: {len(result['instantiations'])}")

    # Print key modules
    for mod in ["Backend_inorder", "EXU", "ALU", "WBU", "NutCore"]:
        if mod in result["modules"]:
            ports = result["modules"][mod]
            print(f"\n{mod}: {len(ports)} port mappings")
            for port, canonical in list(ports.items())[:10]:
                print(f"  {port} -> {canonical}")


if __name__ == "__main__":
    main()

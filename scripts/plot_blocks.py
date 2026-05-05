import json
import matplotlib.pyplot as plt
import numpy as np

# Load data
with open("./blocks.json") as f:
    data = json.load(f)

# Filter data
filtered_sizes = [
    item["block_size"]
    for item in data
    if "block_size" in item
       and item.get("btype") not in ("ModuleInput", "ModuleOutput")
       and item["block_size"] <= 1000
]

sizes = np.array(filtered_sizes)

# Calculate 99th percentile
percentile_99 = np.percentile(sizes, 99)

# Plot histogram with log y-axis
plt.figure(figsize=(10, 2))
plt.hist(sizes, bins=40, color='salmon', edgecolor='black', log=True, alpha=0.7)

# Plot vertical line at 99th percentile
plt.axvline(x=percentile_99, color='red', linestyle='--', linewidth=2, label='99th percentile')

# Set x-axis to start at 0
plt.xlim(left=0, right=max(sizes))

# Titles and labels with larger font size
# plt.title("", fontsize=16)
plt.xlabel("Block Size", fontsize=16, labelpad=-6)
plt.ylabel("#", fontsize=16)
plt.xticks(fontsize=16)
plt.yticks(fontsize=16)

# Increase tick label font size
plt.tick_params(axis='both', which='major', labelsize=12)

plt.legend(fontsize=12)
plt.tight_layout()

# Save figure as PDF
plt.savefig("block_sizes.pdf", format="pdf", dpi=1200, bbox_inches='tight', pad_inches=0.01)

# Show plot
plt.show()

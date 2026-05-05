# venn_plot.py

import matplotlib.pyplot as plt
from matplotlib_venn import venn3

# Define the sets
biosfl = {
    "24", "194", "70", "128", "146", "45", "186", "17", "117",
    "181", "190", "82", "12", "55", "107", "76", "151", "184",
    "98", "48", "init", "195", "28", "42", "20", "88", "138"
}

lik = {"123", "5", "149", "146", "142", "66", "87"}

spectrum = {"159", "155", "197"}

# Create the Venn diagram
venn = venn3(
    [biosfl, lik, spectrum],
    set_labels=('BiosFL', 'LiK', 'Tarsel')
)

# Add title
plt.title("Venn Diagram of Three Approaches")

# Show the plot
plt.show()

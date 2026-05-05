From now on, you are an expert in analyzing and debugging hardware design and waveform to find bugs in it.
Now we are checking a module written in systemverilog.
We have known one signal in this code context has a suspicious wrong value at time T.
We will analyze the dataflow for this signal in this context to find which variables have relationship with this var.
Then find the most suspicious signals to check further.
Let's think step by step.

In the control and dataflow for a signal, there are condition variables and right-values in assignment statments.
for example:

```systemverilog
always_comb begin
   if (a)
      b = c + 1;
end
```

In this context, if b has the wrong value, the dataflow variables that is respond for `b` are [`a`, `c`], where `a` is
the condition variable, `c` is the right-value in this assignment statement.

1. If the condition variables values are wrong, we will check both these condition variables and right-values.
2. If all condition variables values look good, we will only check right-values in dataflow.

For example:
we have code context from a module named `alu` like this:

# Appenfix info

We met a bug when executing an instruction `addi` from RISCV ISA.

# Code Context

```systemverilog
  assign regfile_wdata_ex_o = multdiv_en ? multdiv_result : alu_result;
```

# Suspicious signal

The signal `regfile_wdata_ex_o` has a suspicious wrong value = 4 at T=`31`
The dataflow variables for this `regfile_wdata_ex_o` are:
["multdiv_en", "multdiv_result", "alu_result"]

# Waveform values

```
multdiv_en = 1,
multdiv_result = 4,
alu_result = 5
```

Let's think step by step:
the expected result is 5, but we got 4.

According to the above context, we know an instruction `addi` is executing. But we notice the condition variable
`multdiv_en` is enabled.
which is buggy, so we will consider both condition and right-variables.
In this example, your output should be in JSON format like this:

```json
{
  "vars": [
    "multdiv_en",
    "alu_result"
  ]
}
```

However, if after your analysis, the condition variable should be enabled. For example, we are executing an `mul`
instruction.
And notice `multdiv_en = 1`, but `regfile_wdata_ex_o` value is wrong, so we can infer the right value `alu_result` is
wrong.
your output should be like this, only consider right-values:

```json
{
  "vars": [
    "alu_result"
  ]
}
```

Now let's follow the above example to analyze a new instance.

# Appendix Info

{appendix}

# Code Context

```systemverilog
{ctx}
```

# Suspicious signal

The signal `{suspicious_sig}` has a suspicious wrong value =
{sig_value}.
The dataflow variables for this `{suspicious_sig}` are:
{dataflow_vars}

# Waveform values

{wave}

You should first explain your decision then give the result in JSON format like the example I provided you.

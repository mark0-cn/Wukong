# Worked example: a general-analysis report

User question: "What is the record header, and how does rejection work?"
The analyst's reverse-analysis notes and verifier's checks support this summary:

| Question | Answer | Evidence class |
|---|---|---|
| Where is payload length? | two-byte little-endian field at offset 6 | parser deduction, supported by swapped-length control |
| What protects the payload? | trailing XOR checksum byte | parser deduction, supported by altered-checksum control |
| What happens on a short header? | exit 2 and ERR truncated | independent execution observation |
| Are other record versions supported? | not established by these tests | unresolved outside the tested scope |

In the actual report, every row cites its own original evidence IDs and
sample hash; example IDs or fixture values are never reused as target facts.
Include the field table, sample hex/decoded values, commands/input channel,
expected and actual control results, and the scope of the observations.
There is no vulnerability finding, severity or exploitability conclusion.
Root incorporates the reconciled result into its final assessment. For a delegated
writing task, the analyst returns the contracted artifact to root through its own
completion workflow. The reporting capability does not finish a broader assignment.

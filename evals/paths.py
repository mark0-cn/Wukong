"""Source-controlled fixture data is separate from evaluation programs."""
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent / "fixtures"
ANALYSIS = FIXTURES / "analysis"

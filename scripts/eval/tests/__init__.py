import os
import sys

_here = os.path.dirname(os.path.abspath(__file__))
for p in (_here, os.path.dirname(_here)):
    if p not in sys.path:
        sys.path.insert(0, p)

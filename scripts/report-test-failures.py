"""Expose failed test diagnostics in check annotations, without masking failure."""
import os
import pathlib

path = pathlib.Path(os.environ['RUNNER_TEMP']) / 'flightlog-tests.log'
output = path.read_text(errors='replace')
start = output.find('\nfailures:\n')
detail = output[start:] if start >= 0 else output
# Keep annotations bounded and prevent log content from injecting workflow commands.
detail = detail[-12000:].replace('%', '%25').replace('\r', '%0D').replace('\n', '%0A')
print('::error title=Rust test failures::' + detail)

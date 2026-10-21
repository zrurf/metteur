#!/usr/bin/env python3
"""Read-only verification of this extracted handoff. No installs or network."""
from pathlib import Path
import hashlib,sys
root=Path(__file__).resolve().parent
manifest=root/'SHA256SUMS'
if not manifest.is_file():
 print('Run the copy of this script at the extracted package root.');sys.exit(2)
errors=[];checked=0
for line in manifest.read_text(encoding='utf-8').splitlines():
 try:
  expected,name=line.split('  ',1);target=root/name
  if len(expected)!=64 or any(c not in '0123456789abcdef' for c in expected):raise ValueError('Invalid digest')
  if not target.resolve().is_relative_to(root) or target.is_symlink():raise ValueError('Unsafe path')
  if not target.is_file():raise ValueError('Missing file')
  digest=hashlib.sha256()
  with target.open('rb') as stream:
   for chunk in iter(lambda:stream.read(1024*1024),b''):digest.update(chunk)
  if digest.hexdigest()!=expected:raise ValueError('Digest mismatch')
  checked+=1
 except Exception as exc:errors.append(f'{line[66:]}: {exc}')
if errors:
 print('\n'.join(errors));print(f'FAILED: {len(errors)} errors; {checked} files matched');sys.exit(1)
print(f'PASS: all {checked} listed files match SHA256SUMS. No project code was executed.')

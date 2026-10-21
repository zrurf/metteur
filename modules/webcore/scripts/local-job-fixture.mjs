import { posix, win32 } from 'node:path'

/** Use an actual Node timer so the owned Job fixture works in cmd and sh. */
export function localDelayCommand(milliseconds = 8000) {
  if (!Number.isSafeInteger(milliseconds) || milliseconds < 0 || milliseconds > 60_000)
    throw new RangeError('Fixture delay must be an integer from 0 to 60000 milliseconds')
  return `node -e "setTimeout(() => {}, ${milliseconds})"`
}

/** The daemon must resolve the same Node installation as the fixture launcher. */
export function withCurrentNodePath(environment = process.env, executable = process.execPath, platform = process.platform) {
  const windows = platform === 'win32'
  const paths = windows ? Object.keys(environment).filter(key => key.toLowerCase() === 'path') : ['PATH']
  const key = paths[0] ?? 'Path'
  const value = environment[key] ?? ''
  const result = { ...environment }
  for (const duplicate of paths.slice(1)) delete result[duplicate]
  result[key] = (windows ? win32 : posix).dirname(executable) + (windows ? ';' : ':') + value
  return result
}

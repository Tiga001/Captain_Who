const expectedPlatform = process.argv[2]

if (!expectedPlatform) {
  throw new Error('Expected a Node.js platform name, such as darwin, win32, or linux.')
}

if (process.platform !== expectedPlatform) {
  throw new Error(
    `Native packages must be built on their target OS: requested ${expectedPlatform}, current host ${process.platform}.`
  )
}

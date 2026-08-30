-- Optional extra profile file, referenced from `profile_files`.
-- It may return a bare list, or { profiles = {...}, groups = {...} }.
-- .json / .yaml / .toml files with the same shape work too.
return {
  groups = {
    ['k8s'] = {
      color = '#89b4fa',
      options = { user = 'core' },
    },
  },
  profiles = {
    { name = 'node-a', group = 'k8s', options = { host = '192.0.2.20' } },
    { name = 'node-b', group = 'k8s', options = { host = '192.0.2.21' } },
    {
      name = 'db-primary',
      group = 'k8s',
      options = {
        host = '192.0.2.30',
        forwardedPorts = { 'L 15432:localhost:5432' },
      },
      on_login = { 'sudo -u postgres psql' },
    },
  },
}

import { describe, expect, it } from 'vitest'
import { olderThan } from './version'

describe('olderThan', () => {
  it('orders releases by major.minor.patch', () => {
    expect(olderThan('0.1.0', '0.2.0')).toBe(true)
    expect(olderThan('0.10.0', '0.9.0')).toBe(false)
    expect(olderThan('1.0.0', '1.0.0')).toBe(false)
  })

  it('puts a prerelease before its release', () => {
    expect(olderThan('0.2.0-rc.1', '0.2.0')).toBe(true)
    expect(olderThan('0.2.0', '0.2.0-rc.1')).toBe(false)
    expect(olderThan('0.1.0', '0.2.0-beta.1')).toBe(true)
  })

  it('compares prerelease numbers as numbers', () => {
    expect(olderThan('0.2.0-beta.2', '0.2.0-beta.10')).toBe(true)
    expect(olderThan('0.2.0-beta.10', '0.2.0-beta.2')).toBe(false)
  })

  it('orders prerelease labels as semver does', () => {
    expect(olderThan('0.2.0-beta.9', '0.2.0-rc.1')).toBe(true)
    expect(olderThan('0.2.0-beta', '0.2.0-beta.1')).toBe(true)
    expect(olderThan('0.2.0-1', '0.2.0-beta')).toBe(true)
    expect(olderThan('0.2.0-rc.1', '0.2.0-rc.1')).toBe(false)
  })
})

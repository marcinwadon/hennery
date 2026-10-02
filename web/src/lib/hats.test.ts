import { beforeEach, describe, expect, it } from 'vitest'
import { HAT_KEY, readHat, themeOf, writeHat } from './hats'

beforeEach(() => localStorage.clear())

describe('the selected hat', () => {
  it('is every hat until one is chosen, then remembered', () => {
    expect(readHat()).toBe('')
    writeHat('hat-a')
    expect(localStorage.getItem(HAT_KEY)).toBe('hat-a')
    expect(readHat()).toBe('hat-a')
  })

  it('keys its storage under hennery.', () => {
    expect(HAT_KEY).toBe('hennery.hat')
  })
})

describe('themeOf', () => {
  it('takes a #rrggbb colour, with a darker second stop', () => {
    expect(themeOf('#4C5FD5')).toEqual({ accent: '#4c5fd5', accent2: '#3b49a4' })
  })

  it('refuses anything else: the default colours', () => {
    for (const bad of [undefined, '', 'red', '#abc', '#12345g', '#1234567', 'url(x)', '#112233;color:red']) {
      expect(themeOf(bad)).toBeNull()
    }
  })
})

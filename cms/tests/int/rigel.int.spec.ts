import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'

import { RigelLogo, RigelMark } from '../../src/components/RigelBrand'
import { RigelCloudNav, RigelDashboard } from '../../src/components/RigelCloud'
import {
  RIGEL_CLOUD_LINKS,
  RIGEL_FIREBASE_PROJECT,
  RIGEL_PUBLIC_SITE,
} from '../../src/lib/rigel'

describe('CMS brand parity (roadmap 139)', () => {
  it('links to the public site and the Firebase console without leaving the brand', () => {
    expect(RIGEL_CLOUD_LINKS.some((link) => link.href === RIGEL_PUBLIC_SITE)).toBe(true)
    expect(
      RIGEL_CLOUD_LINKS.some((link) =>
        link.href.includes(`console.firebase.google.com/project/${RIGEL_FIREBASE_PROJECT}`),
      ),
    ).toBe(true)
    for (const link of RIGEL_CLOUD_LINKS) {
      expect(link.href).toMatch(/^https:\/\//)
      expect(link.label.length).toBeGreaterThan(0)
    }
  })

  it('renders both link surfaces with the Rigel mark and no remote images', () => {
    const mark = renderToStaticMarkup(React.createElement(RigelMark, { label: 'Rigel' }))
    expect(mark).toContain('class="rigel-mark"')
    expect(mark).toContain('#e9a23b')
    expect(mark).not.toContain('<img')

    const logo = renderToStaticMarkup(React.createElement(RigelLogo))
    expect(logo).toContain('class="rigel-brand"')
    expect(logo).toContain('Content')

    const nav = renderToStaticMarkup(React.createElement(RigelCloudNav))
    const dashboard = renderToStaticMarkup(React.createElement(RigelDashboard))
    for (const html of [nav, dashboard]) {
      expect(html).toContain(RIGEL_PUBLIC_SITE)
      expect(html).toContain(`console.firebase.google.com/project/${RIGEL_FIREBASE_PROJECT}`)
      expect(html).not.toContain('<img')
      expect(html).toContain('target="_blank"')
      expect(html).toContain('rel="noreferrer"')
    }
  })

  it('keeps the admin stylesheet on Rigel tokens and the Hallmark stamp', () => {
    const fromCms = resolve(process.cwd(), 'src/app/(payload)/custom.scss')
    const stylesheet = existsSync(fromCms)
      ? fromCms
      : resolve(process.cwd(), 'cms/src/app/(payload)/custom.scss')
    const css = readFileSync(stylesheet, 'utf8')
    for (const token of ['--rigel-amber', '--rigel-ink', '--rigel-paper', '--font-mono']) {
      expect(css, `${token} is a Rigel token`).toContain(token)
    }
    expect(css).toContain('Hallmark stamp')
    expect(css).toContain('genre: developer tool')
    expect(css).toContain('theme: instrument dark')
  })
})

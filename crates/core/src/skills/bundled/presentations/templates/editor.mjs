#!/usr/bin/env node
/**
 * Fixed Managed Editor for an existing PowerPoint presentation.
 *
 * Patch only the bounded EDIT REGION. The Host owns source/input resolution,
 * stable-target validation, private staging, validation, and atomic publication.
 */

import { editPresentation, input, output } from '@mycopilot/presentation-sdk'

function requiredValue(flag) {
  const matches = []
  for (let index = 2; index < process.argv.length; index += 1) {
    if (process.argv[index] === flag && process.argv[index + 1]) {
      matches.push(process.argv[index + 1])
      index += 1
    }
  }
  if (matches.length !== 1) throw new Error(`${flag} must be provided exactly once`)
  return matches[0]
}

await editPresentation({
  source: input(requiredValue('--source')),
  destination: output(requiredValue('--output')),
  mode: 'saveAs',
  edit(deck) {
    // BEGIN EDIT REGION
    // Copy stable targets verbatim from the latest `office_presentation` inspect result.
    // Do not invent paths or use positional shape indexes when an inspected id exists.
    //
    // deck.replaceText({
    //   target: '/slide[1]/shape[@id=42]',
    //   find: 'Old title',
    //   replace: 'Updated title'
    // })
    //
    // deck.replaceImage({
    //   target: '/slide[2]/picture[@id=17]',
    //   source: input('media/hero.png')
    // })
    //
    // deck.updateTableCell({
    //   target: '/slide[3]/table[@id=9]/row[2]/cell[3]',
    //   text: '42%'
    // })
    //
    // deck.updateChart({
    //   target: '/slide[4]/chart[@id=11]',
    //   properties: {
    //     categories: ['Q1', 'Q2', 'Q3', 'Q4'],
    //     series: [{ name: 'Revenue', values: [12, 18, 25, 31] }]
    //   }
    // })
    //
    // deck.set({
    //   target: '/slide[5]/shape[@id=23]',
    //   properties: { x: '1in', y: '1.25in' }
    // })
    // deck.add({
    //   parent: '/slide[5]',
    //   elementType: 'textbox',
    //   properties: { text: 'New callout', x: '1in', y: '5in', width: '3in', height: '0.6in' }
    // })
    // deck.move({
    //   target: '/slide[5]/shape[@id=23]',
    //   newParent: '/slide[6]',
    //   position: { type: 'after', target: '/slide[6]/shape[@id=12]' }
    // })
    // deck.swap({
    //   firstTarget: '/slide[6]/shape[@id=12]',
    //   secondTarget: '/slide[6]/shape[@id=14]'
    // })
    // deck.remove({ target: '/slide[8]/shape[@id=31]' })
    // END EDIT REGION
  }
})

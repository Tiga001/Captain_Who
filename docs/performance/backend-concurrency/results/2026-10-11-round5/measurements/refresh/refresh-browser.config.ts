import base from '../../../vitest.config'
const browser = (base.test!.projects![1]) as any
export default { ...base, test: { ...base.test, projects: [{ ...browser, test: { ...browser.test, include: ['.cache/backend-concurrency/round5-validation-20261011/refresh.browser.test.tsx'] } }] } }

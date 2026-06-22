import { ofetch } from 'ofetch'

const api = ofetch.create({ baseURL: '/api/v1' })

export function useApi() {
  return { api }
}

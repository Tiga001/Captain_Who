import hashlib,json,pathlib,re
root=pathlib.Path(__file__).resolve().parent
folder=root/'regressions'
results=[]
for path in sorted(folder.glob('results-*.json')):
    results.extend(json.loads(path.read_text()))
indexes=[item['index'] for item in results]
assert len(indexes)==len(set(indexes)),indexes
expected=set(range(37))
assert set(indexes)==expected,(expected-set(indexes),set(indexes)-expected)
passed=set(); ignored=set(); executions=0; ignored_executions=0
for item in results:
    assert item['returncode']==0,item
    assert item['counts'][0]=='ok' and item['counts'][2]=='0',item
    content=(folder/item['log']).read_text()
    names=re.findall(r'^test ([\w:]+) \.\.\.(.*)$',content,re.M)
    assert len(names)==int(item['counts'][1])+int(item['counts'][3]),item
    for name,suffix in names:
        (ignored if 'ignored' in suffix else passed).add(name)
    executions+=int(item['counts'][1]);ignored_executions+=int(item['counts'][3])
    binary=pathlib.Path('/tmp/captain-backend-baseline-20261011-target/release/deps')/item['binary']
    assert hashlib.sha256(binary.read_bytes()).hexdigest()==item['sha256'],item
result={'suite_count':len(results),'unique_passed_tests':len(passed),'passed_executions':executions,
        'failed_executions':0,'ignored_executions':ignored_executions,'ignored_tests':sorted(ignored),
        'test_names':sorted(passed),'suites':sorted(results,key=lambda i:i['index'])}
p=folder/'accepted-regression-summary.json'
p.write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({k:v for k,v in result.items() if k not in ('test_names','suites')},indent=2))

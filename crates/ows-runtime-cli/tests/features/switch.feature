Feature: switch task

  Scenario: switch routes by condition to a directive
    Given a workflow with definition:
      """
      document:
        dsl: '1.0.3'
        namespace: default
        name: switch
        version: '1.0.0'
      do:
        - pick:
            switch:
              - a:
                  when: '.kind == "a"'
                  then: doA
              - b:
                  when: '.kind == "b"'
                  then: doB
              - default:
                  then: doC
        - doA: { set: { result: 'A' }, then: end }
        - doB: { set: { result: 'B' }, then: end }
        - doC: { set: { result: 'C' }, then: end }
      """
    And given the workflow input is:
      """
      kind: b
      """
    When the workflow is executed
    Then the workflow should complete with output:
      """
      result: B
      """
